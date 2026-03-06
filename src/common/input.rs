use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};

/// Resolves an input path that may be a local file or an HTTP(S) URL.
/// If it's a URL, downloads the file. If the downloaded file is a ZIP,
/// extracts the first file matching the given glob pattern.
/// Returns the path to the resolved local file and whether it's a temp file that should be cleaned up.
pub fn resolve_input(
    input: &Path,
    extract_glob: Option<&str>,
) -> Result<(PathBuf, bool), Box<dyn std::error::Error>> {
    let input_str = input.to_string_lossy();

    if !input_str.starts_with("http://") && !input_str.starts_with("https://") {
        return Ok((input.to_path_buf(), false));
    }

    let url = input_str.as_ref();
    eprintln!("Downloading {url}...");

    let response = ureq::get(url).call()?;
    let content_length = response.headers().get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    let is_zip = url.ends_with(".zip")
        || response.headers().get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct: &str| ct.contains("zip"));

    if is_zip {
        let zip_path = make_temp_path("zip");
        download_to_file(response.into_body().into_reader(), &zip_path, content_length)?;
        let extracted = extract_from_zip(&zip_path, extract_glob)?;
        std::fs::remove_file(&zip_path).ok();
        Ok((extracted, true))
    } else {
        let ext = Path::new(url)
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default();
        let path = make_temp_path(&ext);
        download_to_file(response.into_body().into_reader(), &path, content_length)?;
        Ok((path, true))
    }
}

fn make_temp_path(ext: &str) -> PathBuf {
    let dir = std::env::temp_dir();
    let id = std::process::id();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    if ext.is_empty() {
        dir.join(format!("nominatim-convert-{id}-{ts}.tmp"))
    } else {
        dir.join(format!("nominatim-convert-{id}-{ts}.{ext}"))
    }
}

fn download_to_file(
    mut reader: impl Read,
    path: &Path,
    content_length: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = File::create(path)?;
    let mut downloaded: u64 = 0;
    let mut last_report: u64 = 0;
    let mut buf = vec![0u8; 256 * 1024];

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 { break; }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;

        if downloaded - last_report >= 50_000_000 {
            if let Some(total) = content_length {
                let pct = (downloaded as f64 / total as f64 * 100.0) as u64;
                eprint!("\r  {:.0} MB / {:.0} MB ({pct}%)", downloaded as f64 / 1e6, total as f64 / 1e6);
            } else {
                eprint!("\r  {:.0} MB downloaded", downloaded as f64 / 1e6);
            }
            last_report = downloaded;
        }
    }

    if last_report > 0 {
        eprintln!();
    }
    let size_mb = downloaded as f64 / (1024.0 * 1024.0);
    eprintln!("Downloaded {size_mb:.1} MB to {}", path.display());
    Ok(())
}

fn extract_from_zip(
    zip_path: &Path,
    glob_pattern: Option<&str>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let file = File::open(zip_path)?;
    let reader = BufReader::new(file);
    let mut archive = zip::ZipArchive::new(reader)?;

    let matching_index = (0..archive.len())
        .find(|&i| {
            let Ok(entry) = archive.by_index(i) else { return false };
            let name = entry.name();
            if let Some(pattern) = glob_pattern {
                glob_match(pattern, name)
            } else {
                !name.ends_with('/')
            }
        })
        .ok_or_else(|| {
            let msg = if let Some(p) = glob_pattern {
                format!("No file matching '{p}' found in ZIP")
            } else {
                "ZIP archive is empty".to_string()
            };
            io::Error::new(io::ErrorKind::NotFound, msg)
        })?;

    let mut entry = archive.by_index(matching_index)?;
    let entry_name = entry.name().to_string();

    let ext = Path::new(&entry_name)
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();

    let out_path = make_temp_path(&ext);
    let mut out_file = File::create(&out_path)?;
    io::copy(&mut entry, &mut out_file)?;

    let size_mb = out_file.metadata()?.len() as f64 / (1024.0 * 1024.0);
    eprintln!("Extracted '{entry_name}' ({size_mb:.1} MB)");

    Ok(out_path)
}

/// Simple glob matching supporting only `*` wildcards.
fn glob_match(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        name.ends_with(suffix)
    } else if let Some(prefix) = pattern.strip_suffix('*') {
        name.starts_with(prefix)
    } else {
        name == pattern
    }
}

/// Clean up a resolved input file if it was a temp file.
pub fn cleanup_input(path: &Path, is_temp: bool) {
    if is_temp {
        std::fs::remove_file(path).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match_wildcard_all() {
        assert!(glob_match("*", "anything.txt"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn test_glob_match_suffix() {
        assert!(glob_match("*.csv", "data.csv"));
        assert!(glob_match("*.csv", "path/to/data.csv"));
        assert!(!glob_match("*.csv", "data.xml"));
        assert!(!glob_match("*.csv", "csv"));
    }

    #[test]
    fn test_glob_match_prefix() {
        assert!(glob_match("data*", "data.csv"));
        assert!(glob_match("data*", "data_file.xml"));
        assert!(!glob_match("data*", "other.csv"));
    }

    #[test]
    fn test_glob_match_exact() {
        assert!(glob_match("data.csv", "data.csv"));
        assert!(!glob_match("data.csv", "other.csv"));
    }

    #[test]
    fn test_make_temp_path_with_extension() {
        let path = make_temp_path("csv");
        assert!(path.to_string_lossy().ends_with(".csv"));
        assert!(path.to_string_lossy().contains("nominatim-convert-"));
    }

    #[test]
    fn test_make_temp_path_empty_extension() {
        let path = make_temp_path("");
        assert!(path.to_string_lossy().ends_with(".tmp"));
    }

    #[test]
    fn test_make_temp_path_unique() {
        let p1 = make_temp_path("txt");
        let p2 = make_temp_path("txt");
        assert_ne!(p1, p2);
    }

    #[test]
    fn test_resolve_input_local_file() {
        let path = Path::new("/some/local/file.csv");
        let (resolved, is_temp) = resolve_input(path, Some("*.csv")).unwrap();
        assert_eq!(resolved, path);
        assert!(!is_temp);
    }

    #[test]
    fn test_resolve_input_relative_path() {
        let path = Path::new("relative/file.xml");
        let (resolved, is_temp) = resolve_input(path, None).unwrap();
        assert_eq!(resolved, path);
        assert!(!is_temp);
    }

    #[test]
    fn test_download_to_file() {
        let data = b"hello world test data";
        let reader = io::Cursor::new(data);
        let path = make_temp_path("txt");

        download_to_file(reader, &path, Some(data.len() as u64)).unwrap();

        let contents = std::fs::read(&path).unwrap();
        assert_eq!(contents, data);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_download_to_file_no_content_length() {
        let data = b"some content";
        let reader = io::Cursor::new(data);
        let path = make_temp_path("txt");

        download_to_file(reader, &path, None).unwrap();

        let contents = std::fs::read(&path).unwrap();
        assert_eq!(contents, data);
        std::fs::remove_file(&path).unwrap();
    }

    fn create_test_zip(files: &[(&str, &[u8])]) -> PathBuf {
        let path = make_temp_path("zip");
        let file = File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, content) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(content).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    #[test]
    fn test_extract_from_zip_with_glob() {
        let zip_path = create_test_zip(&[
            ("readme.txt", b"ignore me"),
            ("data.csv", b"col1,col2\na,b"),
        ]);

        let extracted = extract_from_zip(&zip_path, Some("*.csv")).unwrap();
        let contents = std::fs::read_to_string(&extracted).unwrap();
        assert_eq!(contents, "col1,col2\na,b");
        assert!(extracted.to_string_lossy().ends_with(".csv"));

        std::fs::remove_file(&zip_path).unwrap();
        std::fs::remove_file(&extracted).unwrap();
    }

    #[test]
    fn test_extract_from_zip_no_glob_picks_first_file() {
        let zip_path = create_test_zip(&[
            ("first.xml", b"<root/>"),
            ("second.txt", b"text"),
        ]);

        let extracted = extract_from_zip(&zip_path, None).unwrap();
        let contents = std::fs::read_to_string(&extracted).unwrap();
        assert_eq!(contents, "<root/>");

        std::fs::remove_file(&zip_path).unwrap();
        std::fs::remove_file(&extracted).unwrap();
    }

    #[test]
    fn test_extract_from_zip_no_match() {
        let zip_path = create_test_zip(&[
            ("data.xml", b"<root/>"),
        ]);

        let result = extract_from_zip(&zip_path, Some("*.csv"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No file matching"));

        std::fs::remove_file(&zip_path).unwrap();
    }

    #[test]
    fn test_extract_from_zip_skips_directories() {
        let zip_path = create_test_zip(&[
            ("subdir/data.gml", b"<gml/>"),
        ]);

        let extracted = extract_from_zip(&zip_path, Some("*.gml")).unwrap();
        let contents = std::fs::read_to_string(&extracted).unwrap();
        assert_eq!(contents, "<gml/>");

        std::fs::remove_file(&zip_path).unwrap();
        std::fs::remove_file(&extracted).unwrap();
    }

    #[test]
    fn test_cleanup_input_temp_file() {
        let path = make_temp_path("txt");
        File::create(&path).unwrap();
        assert!(path.exists());

        cleanup_input(&path, true);
        assert!(!path.exists());
    }

    #[test]
    fn test_cleanup_input_not_temp() {
        let path = make_temp_path("txt");
        File::create(&path).unwrap();
        assert!(path.exists());

        cleanup_input(&path, false);
        assert!(path.exists());

        std::fs::remove_file(&path).unwrap();
    }
}
