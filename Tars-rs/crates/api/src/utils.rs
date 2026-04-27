use eyre::{eyre, Result};
use reqwest::Url;

/// Joins path segments to a base URL properly handling trailing slashes
///
/// # Arguments
///
/// * `base_url` - The base URL to join segments to
/// * `path_segments` - Array of path segments to join
///
/// # Returns
///
/// A properly constructed URL with all segments joined
pub fn join_url_path(base_url: &Url, path_segments: &[&str]) -> Result<Url> {
    let mut url = base_url.clone();

    // Ensure base URL ends with '/' for proper joining
    if !url.path().ends_with('/') {
        let current_path = url.path().to_string();
        url.set_path(&format!("{}/", current_path));
    }

    // Join all path segments
    for segment in path_segments {
        let trimmed_segment = segment.trim_matches('/');
        url = url
            .join(&format!("{}/", trimmed_segment))
            .map_err(|e| eyre!("Failed to join URL segment '{}': {}", trimmed_segment, e))?;
    }

    // Remove trailing slash from final URL if it exists
    let current_path = url.path().to_string();
    if current_path.ends_with('/') && current_path.len() > 1 {
        url.set_path(&current_path[..current_path.len() - 1]);
    }

    Ok(url)
}
