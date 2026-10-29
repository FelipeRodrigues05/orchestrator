//! `:voice install`: downloads the whisper model and the Piper voice into the data
//! folder. Piper itself is a system package and stays the operator's job.

use std::path::{Path, PathBuf};

use futures::StreamExt;
use tokio::io::AsyncWriteExt;

use super::data_dir;

pub struct Download {
    pub url: String,
    pub path: PathBuf,
}

/// What `:voice install` fetches (about 630 MB).
pub fn downloads() -> Vec<Download> {
    let base = data_dir();
    let piper = "https://huggingface.co/rhasspy/piper-voices/resolve/main/pt/pt_BR/faber/medium";
    vec![
        Download {
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin".into(),
            path: base.join("models/ggml-small.bin"),
        },
        Download {
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin".into(),
            path: base.join("models/ggml-tiny.bin"),
        },
        Download { url: format!("{piper}/pt_BR-faber-medium.onnx"), path: base.join("voices/pt_BR-faber-medium.onnx") },
        Download {
            url: format!("{piper}/pt_BR-faber-medium.onnx.json"),
            path: base.join("voices/pt_BR-faber-medium.onnx.json"),
        },
    ]
}

/// The two files of a Piper voice: `en_US-lessac-medium` lives under
/// `en/en_US/lessac/medium/`. `None` when the id has another shape.
pub fn voice(id: &str) -> Option<Vec<Download>> {
    let (locale, rest) = id.split_once('-')?;
    let (name, quality) = rest.rsplit_once('-')?;
    let family = locale.split_once('_')?.0;
    let valid = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !(valid(locale) && valid(name) && valid(quality) && locale.len() == 5) {
        return None;
    }
    let base =
        format!("https://huggingface.co/rhasspy/piper-voices/resolve/main/{family}/{locale}/{name}/{quality}/{id}");
    let dir = data_dir().join("voices");
    Some(vec![
        Download { url: format!("{base}.onnx"), path: dir.join(format!("{id}.onnx")) },
        Download { url: format!("{base}.onnx.json"), path: dir.join(format!("{id}.onnx.json")) },
    ])
}

/// Fetches `download` unless the file is already there; `progress` gets whole
/// percentages. Writes to a `.part` file first so a broken download never counts.
pub async fn fetch(download: &Download, mut progress: impl FnMut(u8)) -> Result<(), String> {
    if download.path.is_file() {
        return Ok(());
    }
    if let Some(parent) = download.path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    }
    let response = reqwest::get(&download.url).await.map_err(|e| e.without_url().to_string())?;
    if !response.status().is_success() {
        return Err(format!("{} answered {}", download.url, response.status()));
    }
    let total = response.content_length().unwrap_or(0);
    let partial = part_path(&download.path);
    let mut file = tokio::fs::File::create(&partial).await.map_err(|e| e.to_string())?;
    let mut stream = response.bytes_stream();
    let (mut written, mut reported) = (0_u64, 0_u8);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.without_url().to_string())?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        written += chunk.len() as u64;
        if let Some(percent) = (written * 100).checked_div(total).map(|p| p as u8)
            && percent >= reported + 10
        {
            reported = percent - percent % 10;
            progress(reported);
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    tokio::fs::rename(&partial, &download.path).await.map_err(|e| e.to_string())
}

fn part_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_ids_become_piper_urls() {
        let files = voice("en_US-lessac-medium").unwrap();
        assert!(files[0].url.ends_with("/en/en_US/lessac/medium/en_US-lessac-medium.onnx"));
        assert!(files[1].path.ends_with("voices/en_US-lessac-medium.onnx.json"));
        assert!(voice("lessac").is_none() && voice("../x-y-z").is_none());
    }

    #[test]
    fn installs_into_the_folders_the_voice_actor_reads() {
        let downloads = downloads();
        assert!(downloads[0].path.ends_with("models/ggml-small.bin"));
        assert!(downloads[1].path.ends_with("models/ggml-tiny.bin"));
        assert!(downloads[2].path.ends_with("voices/pt_BR-faber-medium.onnx"));
        assert!(downloads.iter().all(|d| d.url.starts_with("https://huggingface.co/")));
    }
}
