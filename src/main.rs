use anyhow::Result;
use blake2b_apple_miner::{config, miner};
use clap::Parser;

fn main() -> Result<()> {
    let args = config::Args::parse();
    let config = config::load(args)?;
    miner::run(config)
}
