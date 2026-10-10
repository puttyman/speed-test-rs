//! Build script.
//!
//! With the `embed-assets` feature it locates the cargo-leptos site output, checks that it
//! has been built, pre-compresses the compressible files (Brotli and gzip) into `OUT_DIR`,
//! and tells `src/server/assets.rs` where everything is through `SPEEDTEST_SITE_DIR`,
//! `SPEEDTEST_SITE_BR_DIR` and `SPEEDTEST_SITE_GZ_DIR` (read by `rust-embed`).
//! Without the feature it does nothing.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(feature = "embed-assets")]
    embed::run();
}

#[cfg(feature = "embed-assets")]
mod embed {
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    /// Extensions worth compressing. Fonts (woff2) and images are already compressed.
    const COMPRESSIBLE: &[&str] = &[
        "wasm",
        "js",
        "mjs",
        "css",
        "html",
        "json",
        "webmanifest",
        "svg",
        "txt",
        "xml",
        "map",
        "ico",
    ];

    pub fn run() {
        println!("cargo:rerun-if-env-changed=SPEEDTEST_SITE_DIR");
        println!("cargo:rerun-if-env-changed=CARGO_TARGET_DIR");

        let site = site_dir();
        let pkg = site.join("pkg");
        if !pkg.is_dir() {
            panic!(
                "\n\nThe `embed-assets` feature embeds the cargo-leptos site output, but `{}` \
                 was not found.\nRun `cargo leptos build --release` first, then build the \
                 server with `--features ssr,embed-assets`.\n(Set SPEEDTEST_SITE_DIR to use a \
                 site directory somewhere else.)\n\n",
                pkg.display()
            );
        }
        // Re-embed whenever cargo-leptos rewrites the site.
        println!("cargo:rerun-if-changed={}", site.display());

        let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo"));
        let br_dir = out.join("site-br");
        let gz_dir = out.join("site-gz");
        for dir in [&br_dir, &gz_dir] {
            if dir.exists() {
                fs::remove_dir_all(dir).expect("clear compressed asset dir");
            }
            fs::create_dir_all(dir).expect("create compressed asset dir");
        }

        let mut files = Vec::new();
        collect_files(&site, &mut files);
        for file in files {
            let rel = file
                .strip_prefix(&site)
                .expect("file is under the site dir");
            let compressible = file
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| COMPRESSIBLE.contains(&e.to_ascii_lowercase().as_str()));
            if !compressible {
                continue;
            }
            let data = fs::read(&file).expect("read site file");
            write_if_smaller(&br_dir.join(rel), &data, brotli(&data));
            write_if_smaller(&gz_dir.join(rel), &data, gzip(&data));
        }

        println!("cargo:rustc-env=SPEEDTEST_SITE_DIR={}", site.display());
        println!("cargo:rustc-env=SPEEDTEST_SITE_BR_DIR={}", br_dir.display());
        println!("cargo:rustc-env=SPEEDTEST_SITE_GZ_DIR={}", gz_dir.display());
    }

    /// cargo-leptos writes the site to `<project>/target/site` (its `site-root` is relative
    /// to the project, so `CARGO_TARGET_DIR` does not move it). `SPEEDTEST_SITE_DIR`
    /// overrides that, and `$CARGO_TARGET_DIR/site` is tried as a fallback.
    fn site_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os("SPEEDTEST_SITE_DIR") {
            return absolute(PathBuf::from(dir));
        }
        let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
        let default = manifest.join("target").join("site");
        if default.join("pkg").is_dir() {
            return default;
        }
        if let Some(target) = std::env::var_os("CARGO_TARGET_DIR") {
            let alt = absolute(PathBuf::from(target)).join("site");
            if alt.join("pkg").is_dir() {
                return alt;
            }
        }
        default
    }

    fn absolute(path: PathBuf) -> PathBuf {
        if path.is_absolute() {
            path
        } else {
            PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join(path)
        }
    }

    fn collect_files(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read site dir") {
            let path = entry.expect("read site dir entry").path();
            if path.is_dir() {
                collect_files(&path, files);
            } else {
                files.push(path);
            }
        }
    }

    /// Keeps a compressed variant only when it saves at least 10%.
    fn write_if_smaller(path: &Path, original: &[u8], compressed: Vec<u8>) {
        if compressed.len() * 10 > original.len() * 9 {
            return;
        }
        fs::create_dir_all(path.parent().unwrap()).expect("create compressed asset dir");
        fs::write(path, compressed).expect("write compressed asset");
    }

    fn brotli(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut writer = brotli::CompressorWriter::new(&mut out, 64 * 1024, 11, 22);
            writer.write_all(data).expect("brotli compress");
        }
        out
    }

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(data).expect("gzip compress");
        encoder.finish().expect("gzip compress")
    }
}
