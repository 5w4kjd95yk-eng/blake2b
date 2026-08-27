use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Copy, Eq, PartialEq)]
enum CudaMode {
    Auto,
    Off,
    Force,
}

fn main() {
    println!("cargo:rerun-if-changed=native/cuda_runtime.cu");
    println!("cargo:rerun-if-env-changed=CUDA_PATH");
    println!("cargo:rerun-if-env-changed=CUDA_HOME");
    println!("cargo:rerun-if-env-changed=CUDA_ARCHITECTURES");
    println!("cargo:rerun-if-env-changed=BLAKE2B_CUDA");
    println!("cargo:rerun-if-env-changed=PATH");
    println!("cargo:rustc-check-cfg=cfg(blake2b_cuda)");

    let mode = cuda_mode();
    if mode == CudaMode::Off {
        return;
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        if mode == CudaMode::Force {
            panic!("CUDA support currently requires a Linux target");
        }
        return;
    }
    if mode == CudaMode::Auto && env::var_os("HOST") != env::var_os("TARGET") {
        println!("cargo:warning=skipping automatic CUDA detection while cross-compiling");
        return;
    }

    let Some(toolkit) = find_cuda_toolkit() else {
        if mode == CudaMode::Force {
            panic!("CUDA toolkit was not found; set CUDA_PATH to its root or set BLAKE2B_CUDA=off");
        }
        println!("cargo:warning=CUDA toolkit not found; building without CUDA support");
        return;
    };
    let nvcc = toolkit.join("bin/nvcc");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let library = out_dir.join("libblake2b_cuda.a");
    let mut command = Command::new(&nvcc);
    command
        .arg("--lib")
        .arg("-o")
        .arg(&library)
        .arg("native/cuda_runtime.cu")
        .arg("-std=c++14")
        // glibc 2.43 exposes C23 rsqrt declarations under _GNU_SOURCE that
        // conflict with CUDA 13.1's device declarations.
        .arg("-U_GNU_SOURCE")
        .arg("-Xcompiler=-fPIC");
    for architecture in architectures() {
        command.arg("-gencode").arg(format!(
            "arch=compute_{architecture},code=sm_{architecture}"
        ));
    }
    let status = command.status();
    if !matches!(status, Ok(status) if status.success()) {
        let detail = status.map_or_else(|error| error.to_string(), |status| status.to_string());
        if mode == CudaMode::Force {
            panic!("{} failed: {detail}", nvcc.display());
        }
        println!(
            "cargo:warning={} failed ({detail}); building without CUDA support",
            nvcc.display()
        );
        return;
    }

    println!("cargo:rustc-cfg=blake2b_cuda");
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=blake2b_cuda");
    println!(
        "cargo:rustc-link-search=native={}",
        cuda_library_dir(&toolkit).display()
    );
    println!("cargo:rustc-link-lib=dylib=cudart");
    println!("cargo:rustc-link-lib=dylib=stdc++");
}

fn cuda_mode() -> CudaMode {
    match env::var("BLAKE2B_CUDA") {
        Ok(value) => match value.as_str() {
            "auto" => CudaMode::Auto,
            "off" => CudaMode::Off,
            "force" => CudaMode::Force,
            _ => panic!("BLAKE2B_CUDA must be auto, off, or force"),
        },
        Err(env::VarError::NotPresent) if env::var_os("CARGO_FEATURE_CUDA").is_some() => {
            CudaMode::Force
        }
        Err(env::VarError::NotPresent) => CudaMode::Auto,
        Err(env::VarError::NotUnicode(_)) => panic!("BLAKE2B_CUDA must contain valid UTF-8"),
    }
}

fn find_cuda_toolkit() -> Option<PathBuf> {
    env::var_os("CUDA_PATH")
        .or_else(|| env::var_os("CUDA_HOME"))
        .map(PathBuf::from)
        .or_else(|| {
            find_in_path("nvcc")
                .and_then(|nvcc| fs::canonicalize(nvcc).ok())
                .and_then(|nvcc| nvcc.parent()?.parent().map(Path::to_owned))
        })
        .or_else(|| {
            ["/usr/local/cuda", "/opt/cuda"]
                .into_iter()
                .map(PathBuf::from)
                .find(|root| root.join("bin/nvcc").is_file())
        })
        .filter(|root| root.join("bin/nvcc").is_file())
}

fn find_in_path(program: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join(program))
        .find(|path| path.is_file())
}

fn architectures() -> Vec<String> {
    let raw = env::var("CUDA_ARCHITECTURES").unwrap_or_else(|_| "75,80,86,89,120".to_owned());
    let values = raw
        .split([',', ';', ' '])
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_start_matches("sm_").to_owned())
        .collect::<Vec<_>>();
    assert!(!values.is_empty(), "CUDA_ARCHITECTURES must not be empty");
    assert!(
        values
            .iter()
            .all(|value| value.chars().all(|character| character.is_ascii_digit())),
        "CUDA_ARCHITECTURES entries must be numbers such as 75,86,89"
    );
    values
}

fn cuda_library_dir(toolkit: &Path) -> PathBuf {
    let target_dir = toolkit.join("targets/x86_64-linux/lib");
    if target_dir.is_dir() {
        target_dir
    } else {
        toolkit.join("lib64")
    }
}
