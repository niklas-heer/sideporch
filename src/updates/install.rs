//! Replacing the running program with a newer release.

use std::path::Path;

/// How Sideporch was installed, which decides how it updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// From a release archive, by the install script or by hand: replace the file.
    Archive,
    /// Homebrew manages the file.
    Homebrew,
    /// A read-only Nix store path.
    Nix,
    /// A container image; a new image replaces it.
    Container,
}

impl Method {
    /// Tells from where the program lives and whether it runs in a container.
    #[must_use]
    pub fn detect(executable: &Path, in_container: bool) -> Self {
        let path = executable.to_string_lossy();
        if path.starts_with("/nix/store/") {
            Self::Nix
        } else if path.contains("/Cellar/") || path.contains("/linuxbrew/") {
            Self::Homebrew
        } else if in_container {
            Self::Container
        } else {
            Self::Archive
        }
    }

    /// How to update an installation like this one by hand.
    #[must_use]
    pub const fn advice(self) -> &'static str {
        match self {
            Self::Archive => {
                "Run `sudo sideporch update`, then restart Sideporch (`sudo systemctl restart sideporch`)."
            }
            Self::Homebrew => {
                "Run `brew upgrade sideporch`, then `brew services restart sideporch`."
            }
            Self::Nix => "Update the flake input (`nix flake update sideporch`) and rebuild.",
            Self::Container => {
                "Pull the new image (`docker pull ghcr.io/niklas-heer/sideporch`) and recreate the container."
            }
        }
    }
}

/// The release archive for `target`, as the release names it.
#[must_use]
pub fn archive_name(version: &str, target: &str) -> String {
    // Releases ship static musl builds for Linux, whatever this one is.
    let target = match target.split_once("-unknown-linux-") {
        Some((arch, _)) => format!("{arch}-unknown-linux-musl"),
        None => target.to_owned(),
    };
    format!("sideporch-{version}-{target}.tar.gz")
}

/// The checksum `SHA256SUMS` lists for `file`.
#[must_use]
pub fn checksum_for<'a>(sums: &'a str, file: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let (sum, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim().trim_start_matches('*').trim_start_matches("./");
        (name == file).then_some(sum.trim())
    })
}

/// Writes the `sideporch` program from a release archive to `target`.
///
/// # Errors
///
/// Fails if the archive can't be read, has no program, or `target` can't
/// be written.
pub fn extract(archive: &Path, target: &Path) -> Result<(), String> {
    use std::io::Read as _;
    const MAX_PROGRAM: u64 = 512 * 1024 * 1024;
    let file = std::fs::File::open(archive).map_err(|error| error.to_string())?;
    let mut entries = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let entries = entries
        .entries()
        .map_err(|_| "the release archive can't be read".to_owned())?;
    for entry in entries {
        let entry = entry.map_err(|_| "the release archive can't be read".to_owned())?;
        let path = entry
            .path()
            .map_err(|_| "the release archive can't be read".to_owned())?;
        let at_top_or_one_down = path.components().count() <= 2;
        if !(entry.header().entry_type().is_file()
            && at_top_or_one_down
            && path.file_name().is_some_and(|name| name == "sideporch"))
        {
            continue;
        }
        let mut program = Vec::new();
        entry
            .take(MAX_PROGRAM)
            .read_to_end(&mut program)
            .map_err(|_| "the release archive can't be read".to_owned())?;
        write_program(target, &program)?;
        return Ok(());
    }
    Err("the release archive has no sideporch program".to_owned())
}

/// Writes an executable file, readable and runnable by everyone.
fn write_program(target: &Path, program: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o755);
    let mut file = options.open(target).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    std::fs::set_permissions(target, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .map_err(|error| error.to_string())?;
    file.write_all(program).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_how_it_was_installed() {
        let archive = [
            ("/usr/local/bin/sideporch", false),
            ("/home/ada/.local/bin/sideporch", false),
            ("/opt/sideporch/bin/sideporch", false),
        ];
        for (path, container) in archive {
            assert_eq!(
                Method::detect(Path::new(path), container),
                Method::Archive,
                "{path}"
            );
        }
        assert_eq!(
            Method::detect(
                Path::new("/opt/homebrew/Cellar/sideporch/0.4.1/bin/sideporch"),
                false
            ),
            Method::Homebrew
        );
        assert_eq!(
            Method::detect(
                Path::new("/home/linuxbrew/.linuxbrew/Cellar/sideporch/0.4.1/bin/sideporch"),
                false
            ),
            Method::Homebrew
        );
        assert_eq!(
            Method::detect(
                Path::new("/nix/store/abc-sideporch-0.4.1/bin/sideporch"),
                false
            ),
            Method::Nix
        );
        assert_eq!(
            Method::detect(Path::new("/sideporch"), true),
            Method::Container
        );
    }

    #[test]
    fn names_the_archive_for_this_system() {
        assert_eq!(
            archive_name("0.5.0", "aarch64-unknown-linux-musl"),
            "sideporch-0.5.0-aarch64-unknown-linux-musl.tar.gz"
        );
        // Linux builds of any kind update to the static release build.
        assert_eq!(
            archive_name("0.5.0", "x86_64-unknown-linux-gnu"),
            "sideporch-0.5.0-x86_64-unknown-linux-musl.tar.gz"
        );
        assert_eq!(
            archive_name("0.5.0", "aarch64-apple-darwin"),
            "sideporch-0.5.0-aarch64-apple-darwin.tar.gz"
        );
    }

    #[test]
    fn finds_checksums() {
        let sums = "aaa  sideporch-0.5.0-aarch64-apple-darwin.tar.gz\nbbb  ./sideporch-0.5.0-x86_64-unknown-linux-musl.tar.gz\n";
        assert_eq!(
            checksum_for(sums, "sideporch-0.5.0-aarch64-apple-darwin.tar.gz"),
            Some("aaa")
        );
        assert_eq!(
            checksum_for(sums, "sideporch-0.5.0-x86_64-unknown-linux-musl.tar.gz"),
            Some("bbb")
        );
        assert_eq!(
            checksum_for(sums, "sideporch-0.5.0-x86_64-apple-darwin.tar.gz"),
            None
        );
    }

    /// A release-style archive: a directory holding `sideporch` and a README.
    pub fn archive_with(program: &[u8]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let gz = flate2::write::GzEncoder::new(file.reopen().unwrap(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        for (name, data) in [
            ("sideporch-0.5.0/README.md", b"read me".as_slice()),
            ("sideporch-0.5.0/sideporch", program),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(u64::try_from(data.len()).unwrap());
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, name, data).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        file
    }

    #[test]
    fn extracts_the_program() {
        let archive = archive_with(b"#!/bin/sh\necho sideporch 0.5.0\n");
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("new");
        extract(archive.path(), &target).unwrap();
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"#!/bin/sh\necho sideporch 0.5.0\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
        let empty = tempfile::NamedTempFile::new().unwrap();
        assert!(extract(empty.path(), &target).is_err());
    }
}
