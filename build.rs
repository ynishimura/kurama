//! Embed the signed, version-matched httpfs extension and compile the
//! exception boundary. The case tests are not generated here: `cargo xtask
//! generate-cases` writes them to a committed file, so editing a case does
//! not rerun this script and rebuild the whole package.
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

const HTTPFS_VERSION: &str = "1.5.3";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/adapters/duckdb/arrow_bridge.cpp");
    println!("cargo:rerun-if-env-changed=KURAMA_HTTPFS_ARCHIVE");
    println!("cargo:rustc-env=KURAMA_HTTPFS_VERSION={HTTPFS_VERSION}");
    let include =
        PathBuf::from(env::var_os("DEP_DUCKDB_LIB_DIR").expect("bundled DuckDB library directory"))
            .join("duckdb/src/include");
    cc::Build::new()
        .cpp(true)
        .std("c++11")
        .include(include)
        .file("src/adapters/duckdb/arrow_bridge.cpp")
        .compile("kurama_arrow_bridge");
    let (platform, hash) = match env::var("TARGET").unwrap().as_str() {
        "aarch64-apple-darwin" => (
            "osx_arm64",
            "d328b23388b9e74081a751c52a43430df83a9fba9fc2dd76db6a4a7c98430486",
        ),
        "x86_64-apple-darwin" => (
            "osx_amd64",
            "f10c3394cfc0ee3712104e366a4c748d5f0f79222f34478e0028187d6fa95e58",
        ),
        "x86_64-unknown-linux-gnu" => (
            "linux_amd64",
            "e33a4085f04b84bcbb25c27dec3a821731e4d2476b93cfc84aa46c963098b882",
        ),
        "aarch64-unknown-linux-gnu" => (
            "linux_arm64",
            "bde75aadc4ebf9edb4b9fecf4aac6025f35fdae56116b2673b0b4990242c3a02",
        ),
        target => panic!("httpfs is not packaged for {target}"),
    };
    let directory = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let output = directory.join(format!(
        "httpfs-{HTTPFS_VERSION}-{platform}.duckdb_extension.gz"
    ));
    let supplied = env::var_os("KURAMA_HTTPFS_ARCHIVE").map(PathBuf::from);
    if let Some(archive) = &supplied {
        println!("cargo:rerun-if-changed={}", archive.display());
    }
    if supplied.is_some() || !matches_hash(&output, hash) {
        let partial = output.with_extension("part");
        let legacy = directory.join("httpfs.duckdb_extension.gz");
        let copy = supplied
            .as_deref()
            .or_else(|| matches_hash(&legacy, hash).then_some(legacy.as_path()));
        if let Some(archive) = copy {
            fs::copy(archive, &partial).expect("copy httpfs archive to staging file");
        } else {
            let url = format!(
                "https://extensions.duckdb.org/v{HTTPFS_VERSION}/{platform}/httpfs.duckdb_extension.gz"
            );
            let status = Command::new("curl")
                .args([
                    "--fail",
                    "--silent",
                    "--show-error",
                    "--location",
                    "--proto",
                    "=https",
                    "--proto-redir",
                    "=https",
                    "--max-time",
                    "120",
                    "--output",
                ])
                .arg(&partial)
                .arg(url)
                .status()
                .expect("curl is needed at build time, or set KURAMA_HTTPFS_ARCHIVE");
            if !status.success() {
                let _ = fs::remove_file(&partial);
                panic!("httpfs download failed; set KURAMA_HTTPFS_ARCHIVE for an offline build");
            }
        }
        if !matches_hash(&partial, hash) {
            let _ = fs::remove_file(&partial);
            panic!(
                "httpfs SHA-256 mismatch for {}; staged archive discarded; retry the build or provide KURAMA_HTTPFS_ARCHIVE",
                output.display()
            );
        }
        fs::rename(&partial, &output).expect("publish verified httpfs archive");
    }
    println!(
        "cargo:rustc-env=KURAMA_HTTPFS_ARCHIVE_PATH={}",
        output.display()
    );
}

fn matches_hash(path: &Path, expected: &str) -> bool {
    fs::read(path).is_ok_and(|bytes| {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            == expected
    })
}
