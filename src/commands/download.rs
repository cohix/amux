use crate::cli::Agent;
use crate::commands::output::OutputSink;
use anyhow::{bail, Context, Result};
use std::path::Path;

/// Base URL for raw file downloads from GitHub.
const ASPEC_CLI_RAW_BASE: &str =
    "https://raw.githubusercontent.com/cohix/aspec-cli/main/templates";

/// URL for downloading the aspec repo tarball.
const ASPEC_REPO_TARBALL: &str =
    "https://api.github.com/repos/cohix/aspec/tarball/main";

/// Download a Dockerfile template for the given agent from GitHub.
///
/// Returns the template content as a string.
/// Logs the download URL and result size to `out`.
pub async fn download_dockerfile_template(
    agent: &Agent,
    out: &OutputSink,
) -> Result<String> {
    let filename = match agent {
        Agent::Claude => "Dockerfile.claude",
        Agent::Codex => "Dockerfile.codex",
        Agent::Opencode => "Dockerfile.opencode",
    };
    let url = format!("{}/{}", ASPEC_CLI_RAW_BASE, filename);

    out.println(format!("Downloading {} from {}", filename, url));

    let content = download_text(&url).await
        .with_context(|| format!("Failed to download {}", url))?;

    out.println(format!(
        "Downloaded {} ({} bytes)",
        filename,
        content.len()
    ));

    Ok(content)
}

/// Download the `aspec/` folder from the aspec GitHub repo into `dest_dir`.
///
/// Downloads the repo tarball, extracts only the `aspec/` subdirectory,
/// and copies it to `dest_dir/aspec/`.
/// Logs progress and result to `out`.
pub async fn download_aspec_folder(
    dest_dir: &Path,
    out: &OutputSink,
) -> Result<()> {
    out.println(format!(
        "Downloading aspec folder from {}",
        ASPEC_REPO_TARBALL
    ));

    let bytes = download_bytes(ASPEC_REPO_TARBALL).await
        .context("Failed to download aspec repo tarball")?;

    out.println(format!(
        "Downloaded tarball ({} bytes), extracting aspec/ folder...",
        bytes.len()
    ));

    let aspec_dest = dest_dir.join("aspec");
    extract_aspec_from_tarball(&bytes, &aspec_dest)?;

    // Count files extracted.
    let file_count = count_files_recursive(&aspec_dest)?;
    out.println(format!(
        "Extracted aspec folder to {} ({} files)",
        aspec_dest.display(),
        file_count
    ));

    Ok(())
}

/// Download a URL and return the response body as text.
async fn download_text(url: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .user_agent("aspec-cli")
        .build()?;
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        bail!(
            "HTTP {} when downloading {}",
            resp.status(),
            url
        );
    }
    Ok(resp.text().await?)
}

/// Download a URL and return the response body as bytes.
async fn download_bytes(url: &str) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .user_agent("aspec-cli")
        .build()?;
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        bail!(
            "HTTP {} when downloading {}",
            resp.status(),
            url
        );
    }
    Ok(resp.bytes().await?.to_vec())
}

/// Extract the `aspec/` directory from a gzipped tarball into `dest`.
///
/// The tarball from GitHub has a top-level directory like `cohix-aspec-<sha>/`.
/// We look for entries under `<top>/aspec/` and strip that prefix.
pub fn extract_aspec_from_tarball(tarball_bytes: &[u8], dest: &Path) -> Result<()> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let decoder = GzDecoder::new(tarball_bytes);
    let mut archive = Archive::new(decoder);

    // First pass: find the top-level directory name and extract aspec/ entries.
    let mut extracted_count = 0u64;

    for entry in archive.entries().context("Failed to read tarball entries")? {
        let mut entry = entry.context("Failed to read tarball entry")?;
        let path = entry.path().context("Failed to read entry path")?.into_owned();
        let path_str = path.to_string_lossy().to_string();

        // GitHub tarballs have format: <owner>-<repo>-<sha>/aspec/...
        // Find the first `/` to get the top-level dir, then look for /aspec/ after it.
        let components: Vec<&str> = path_str.split('/').collect();
        if components.len() < 2 {
            continue;
        }

        // Check if the second component is "aspec"
        if components[1] != "aspec" {
            continue;
        }

        // Build the relative path within the aspec/ directory.
        // components[0] = top-level dir, components[1] = "aspec", rest = subpath
        let relative: String = components[2..].join("/");

        if relative.is_empty() {
            // This is the aspec/ directory itself.
            std::fs::create_dir_all(dest)
                .with_context(|| format!("Failed to create {}", dest.display()))?;
            continue;
        }

        let target = dest.join(&relative);

        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&target)
                .with_context(|| format!("Failed to create directory {}", target.display()))?;
        } else {
            // Ensure parent directory exists.
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            entry
                .unpack(&target)
                .with_context(|| format!("Failed to extract {}", target.display()))?;
            extracted_count += 1;
        }
    }

    if extracted_count == 0 {
        bail!("No aspec/ files found in the downloaded tarball");
    }

    Ok(())
}

/// Count files recursively in a directory.
fn count_files_recursive(dir: &Path) -> Result<usize> {
    let mut count = 0;
    if !dir.exists() {
        return Ok(0);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_file() {
            count += 1;
        } else if ft.is_dir() {
            count += count_files_recursive(&entry.path())?;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_aspec_from_tarball_works() {
        // Build a minimal gzip tarball with the expected structure.
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let tmp = tempfile::TempDir::new().unwrap();

        // Create an in-memory tarball.
        let mut tar_data = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_data);

            // Add a directory entry: cohix-aspec-abc123/aspec/
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "cohix-aspec-abc123/aspec/", &[] as &[u8])
                .unwrap();

            // Add a file: cohix-aspec-abc123/aspec/foundation.md
            let content = b"# Project Foundation\nTest content\n";
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    "cohix-aspec-abc123/aspec/foundation.md",
                    &content[..],
                )
                .unwrap();

            // Add a file in a subdirectory: cohix-aspec-abc123/aspec/work-items/0000-template.md
            let content2 = b"# Work Item: template\n";
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(content2.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    "cohix-aspec-abc123/aspec/work-items/0000-template.md",
                    &content2[..],
                )
                .unwrap();

            // Add a non-aspec file that should be skipped.
            let content3 = b"README\n";
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(content3.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    "cohix-aspec-abc123/README.md",
                    &content3[..],
                )
                .unwrap();

            builder.finish().unwrap();
        }

        // Gzip the tar data.
        let mut gz_data = Vec::new();
        {
            let mut encoder = GzEncoder::new(&mut gz_data, Compression::default());
            encoder.write_all(&tar_data).unwrap();
            encoder.finish().unwrap();
        }

        let dest = tmp.path().join("aspec");
        extract_aspec_from_tarball(&gz_data, &dest).unwrap();

        // Verify extracted files.
        assert!(dest.join("foundation.md").exists());
        assert!(dest.join("work-items/0000-template.md").exists());

        let content = std::fs::read_to_string(dest.join("foundation.md")).unwrap();
        assert!(content.contains("Project Foundation"));

        // Verify non-aspec file was NOT extracted.
        assert!(!dest.join("README.md").exists());
    }

    #[test]
    fn extract_aspec_from_tarball_empty_fails() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let tmp = tempfile::TempDir::new().unwrap();

        // Create an empty tarball.
        let mut tar_data = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_data);
            builder.finish().unwrap();
        }

        let mut gz_data = Vec::new();
        {
            let mut encoder = GzEncoder::new(&mut gz_data, Compression::default());
            encoder.write_all(&tar_data).unwrap();
            encoder.finish().unwrap();
        }

        let dest = tmp.path().join("aspec");
        let result = extract_aspec_from_tarball(&gz_data, &dest);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No aspec/ files found"));
    }

    #[test]
    fn count_files_recursive_counts_correctly() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "a").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "b").unwrap();
        let sub = tmp.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("c.txt"), "c").unwrap();

        assert_eq!(count_files_recursive(tmp.path()).unwrap(), 3);
    }

    #[test]
    fn count_files_recursive_nonexistent_returns_zero() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent");
        assert_eq!(count_files_recursive(&path).unwrap(), 0);
    }
}
