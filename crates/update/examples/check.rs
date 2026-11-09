//! Dev tool: checks GitHub for updates as if this build were `[version]` (default: this build),
//! twice (the second is a conditional request), and with `--download` downloads and verifies the
//! installer without running it.
//!
//! `cargo run -p kuality-update --example check -- 1.0.0-alpha --download`

use kuality_update::{Checker, Version, can_self_update, current_version, download_verified};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let download = args.iter().any(|a| a == "--download");
    let current = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(|v| Version::parse(v).expect("invalid version"))
        .unwrap_or_else(current_version);
    println!(
        "as version {current}; installed copy: {}",
        can_self_update()
    );

    let mut checker = Checker::new();
    for round in 1..=2 {
        let start = std::time::Instant::now();
        let result = checker.check_from(&current);
        println!("check {round} ({:?}): {result:?}", start.elapsed());
        if round == 2
            && download
            && let Ok(Some(release)) = result
        {
            let start = std::time::Instant::now();
            match download_verified(&release) {
                Ok(path) => println!("verified {} ({:?})", path.display(), start.elapsed()),
                Err(e) => println!("download failed: {e}"),
            }
        }
    }
}
