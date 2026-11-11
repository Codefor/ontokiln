use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Command};

fn collect(root: &Path, paths: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(root).expect("kernel sources").flatten() {
        let path = entry.path();
        if path.is_dir() && !["target", ".git"].contains(&entry.file_name().to_str().unwrap_or(""))
        {
            println!("cargo:rerun-if-changed={}", path.display());
            collect(&path, paths);
        } else if path.extension().is_some_and(|e| e == "rs")
            || path.file_name().is_some_and(|n| n == "Cargo.toml")
        {
            paths.push(path);
        }
    }
}

fn main() {
    let mut paths = vec![
        "Cargo.toml".into(),
        "Cargo.lock".into(),
        "build.rs".into(),
        "rust-toolchain.toml".into(),
    ];
    collect(Path::new("src"), &mut paths);
    collect(Path::new("crates"), &mut paths);
    collect(Path::new("vendor/spareval"), &mut paths);
    paths.sort();
    let mut hasher = Sha256::new();
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path.to_string_lossy().replace('\\', "/");
        let bytes = fs::read(&path).expect("read kernel source");
        hasher.update((name.len() as u64).to_be_bytes());
        hasher.update(name.as_bytes());
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    let features = std::env::vars()
        .filter(|(name, _)| name.starts_with("CARGO_FEATURE_"))
        .collect::<std::collections::BTreeMap<_, _>>();
    for (name, value) in features {
        hasher.update(name.as_bytes());
        hasher.update(value.as_bytes());
    }
    println!(
        "cargo:rustc-env=ONTOKILN_KERNEL_SHA256={:x}",
        hasher.finalize()
    );
    println!("cargo:rerun-if-env-changed=ONTOKILN_BUILD_REVISION");
    // HEAD and its symbolic ref can change without changing any kernel source.
    for file in ["HEAD", "packed-refs"] {
        let result = Command::new("git")
            .args(["rev-parse", "--git-path", file])
            .output()
            .expect("Git path");
        println!(
            "cargo:rerun-if-changed={}",
            String::from_utf8_lossy(&result.stdout).trim()
        );
    }
    let result = Command::new("git")
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .expect("Git ref");
    if result.status.success() {
        let reference = String::from_utf8_lossy(&result.stdout).trim().to_owned();
        let result = Command::new("git")
            .args(["rev-parse", "--git-path", &reference])
            .output()
            .expect("Git ref path");
        println!(
            "cargo:rerun-if-changed={}",
            String::from_utf8_lossy(&result.stdout).trim()
        );
    }
    let revision = std::env::var("ONTOKILN_BUILD_REVISION").unwrap_or_else(|_| {
        let result = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git revision");
        assert!(
            result.status.success(),
            "build outside Git requires ONTOKILN_BUILD_REVISION"
        );
        String::from_utf8(result.stdout)
            .expect("revision UTF-8")
            .trim()
            .to_owned()
    });
    assert!(
        revision.len() == 40
            && revision
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "full source SHA required"
    );
    println!("cargo:rustc-env=ONTOKILN_BUILD_REVISION={revision}");
}
