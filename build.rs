//! Generates the stylesheet with encre-css, a Rust implementation of
//! Tailwind, so the build needs no Node toolchain.

use std::{
    collections::hash_map::DefaultHasher,
    env, fs,
    hash::{Hash, Hasher},
    io,
    path::{Path, PathBuf},
};

use encre_css::{Config, preflight::Preflight};

const SANS: &str = "\"Atkinson Hyperlegible Next\", ui-sans-serif, system-ui, sans-serif, \"Apple Color Emoji\", \"Segoe UI Emoji\"";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo::rerun-if-changed=src");
    println!("cargo::rerun-if-changed=assets");
    println!("cargo::rerun-if-changed=encre-css.toml");

    let mut sources = Vec::new();
    collect(Path::new("src"), "rs", &mut sources)?;
    collect(Path::new("assets"), "js", &mut sources)?;
    let contents = sources
        .iter()
        .map(fs::read_to_string)
        .collect::<io::Result<Vec<_>>>()?;

    let mut config = Config::from_file("encre-css.toml")?;
    config.preflight = Preflight::new_full().font_family_sans(SANS);
    let utilities = encre_css::generate(contents.iter().map(String::as_str), &config);
    let css = format!("{}\n{utilities}\n", fs::read_to_string("assets/base.css")?);

    let out = PathBuf::from(env::var("OUT_DIR")?);
    fs::write(out.join("app.css"), &css)?;

    let mut hasher = DefaultHasher::new();
    css.hash(&mut hasher);
    fs::read_to_string("assets/app.js")?.hash(&mut hasher);
    println!(
        "cargo::rustc-env=SIDEPORCH_ASSET_VERSION={:x}",
        hasher.finish()
    );
    Ok(())
}

fn collect(dir: &Path, extension: &str, found: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(&path, extension, found)?;
        } else if path.extension().is_some_and(|ext| ext == extension) {
            found.push(path);
        }
    }
    found.sort();
    Ok(())
}
