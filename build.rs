use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=native/cuda_runtime.cu");
    println!("cargo:rerun-if-env-changed=CUDA_PATH");
    println!("cargo:rerun-if-env-changed=CUDA_HOME");
    println!("cargo:rerun-if-env-changed=CUDA_ARCHITECTURES");

    if env::var_os("CARGO_FEATURE_CUDA").is_none() {
        return;
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        panic!("the CUDA feature currently supports Linux only");
    }

    let toolkit = env::var_os("CUDA_PATH")
        .or_else(|| env::var_os("CUDA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/local/cuda"));
    let nvcc = toolkit.join("bin/nvcc");
    if !nvcc.is_file() {
        panic!(
            "nvcc was not found at {}; set CUDA_PATH to the CUDA toolkit root",
            nvcc.display()
        );
    }

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
    let status = command.status().unwrap_or_else(|error| {
        panic!("failed to run {}: {error}", nvcc.display());
    });
    assert!(status.success(), "nvcc failed with status {status}");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=blake2b_cuda");
    println!(
        "cargo:rustc-link-search=native={}",
        cuda_library_dir(&toolkit).display()
    );
    println!("cargo:rustc-link-lib=dylib=cudart");
    println!("cargo:rustc-link-lib=dylib=stdc++");
}

fn architectures() -> Vec<String> {
    let raw =
        env::var("CUDA_ARCHITECTURES").unwrap_or_else(|_| "75,80,86,89,120".to_owned());
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
