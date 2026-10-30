// src/engines/audio_engine/store/local_store.rs

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::engines::audio_engine::store::store_api_client::{
    AudioAlbum, AudioCollection, AudioContributor, AudioLanguage, ModuleStatus,
    RemoteAudioModuleInfo,
};

// ─────────────────────────────────────────────────────────────────────────────
// Path segments
// ─────────────────────────────────────────────────────────────────────────────

pub fn sanitize_segment(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "_".to_string()
    } else if trimmed.chars().count() > 120 {
        trimmed.chars().take(120).collect()
    } else {
        trimmed
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct ModuleLocation {
    pub language_code: String,
    pub language_name: String,
    pub contributor_name: Option<String>,
    pub collection_name: String,
    pub album_title: String,
    pub module_unique_id: String,
    pub module_version: i32,
}

impl ModuleLocation {
    pub fn folder_relative(&self) -> PathBuf {
        let mut p = PathBuf::new();
        p.push(sanitize_segment(&self.language_code));
        if let Some(ref c) = self.contributor_name {
            let s = sanitize_segment(c);
            if !s.is_empty() && s != "_" {
                p.push(s);
            }
        }
        p.push(sanitize_segment(&self.collection_name));
        p.push(sanitize_segment(&self.album_title));
        p
    }

    pub fn file_name(&self) -> String {
        format!(
            "{}_v{}.xba",
            sanitize_segment(&self.module_unique_id),
            self.module_version
        )
    }

    pub fn from_hierarchy(
        language: &AudioLanguage,
        collection: &AudioCollection,
        album: &AudioAlbum,
        contributor: Option<&AudioContributor>,
        module: &RemoteAudioModuleInfo,
    ) -> Self {
        Self {
            language_code: language.code.clone(),
            language_name: language.name.clone(),
            contributor_name: contributor.map(|c| c.name.clone()),
            collection_name: collection.name.clone(),
            album_title: album.title.clone(),
            module_unique_id: module.unique_id.clone(),
            module_version: module.version,
        }
    }
}

pub fn module_path(root: &Path, loc: &ModuleLocation) -> PathBuf {
    root.join(loc.folder_relative()).join(loc.file_name())
}

// ─────────────────────────────────────────────────────────────────────────────
// Metadata shapes
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XbaMetadataStub {
    pub unique_id: String,
    pub display_title: String,
    pub version: i32,
    pub language: String,
    pub contributor: String,
    pub description: String,
    pub source_url: String,
    pub duration_ms: i64,
    pub features: Vec<String>,
    #[serde(default)]
    pub artwork_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalOrganization {
    pub language_code: String,
    pub language_name: String,
    pub contributor_name: Option<String>,
    pub collection_id: String,
    pub collection_name: String,
    pub collection_description: Option<String>,
    pub album_id: String,
    pub album_title: String,
    pub album_manifest_version: Option<String>,
    pub album_order: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contributor_artwork_entry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artwork_entry: Option<String>,
    pub stamped_at: String,
    pub stamped_by_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XbaMetadata {
    #[serde(flatten)]
    pub stub: XbaMetadataStub,
    pub organization: LocalOrganization,
}

impl XbaMetadata {
    pub fn stamp(
        stub: XbaMetadataStub,
        location: &ModuleLocation,
        collection: &AudioCollection,
        album: &AudioAlbum,
    ) -> Self {
        Self {
            stub,
            organization: LocalOrganization {
                language_code: location.language_code.clone(),
                language_name: location.language_name.clone(),
                contributor_name: location.contributor_name.clone(),
                collection_id: collection.id.clone(),
                collection_name: collection.name.clone(),
                collection_description: collection.description.clone(),
                album_id: album.id.clone(),
                album_title: album.title.clone(),
                album_manifest_version: album.manifest_version.clone(),
                album_order: album.order,
                contributor_artwork_entry: None,
                album_artwork_entry: None,
                stamped_at: now_rfc3339(),
                stamped_by_version: env!("CARGO_PKG_VERSION").to_string(),
            },
        }
    }
}

fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let days = (secs / 86_400) as i64;
    let sod = (secs % 86_400) as u32;

    // Time-of-day components (hour, minute, second)
    let hour = sod / 3600;
    let minute = (sod % 3600) / 60;
    let second = sod % 60;

    // Civil-from-days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, minute, second
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Archive readers
// ─────────────────────────────────────────────────────────────────────────────

pub fn read_stub_metadata(xba_path: &Path) -> std::io::Result<XbaMetadataStub> {
    let file = std::fs::File::open(xba_path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut entry = zip.by_name("metadata.json").map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "metadata.json not in xba")
    })?;
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf)?;
    serde_json::from_slice(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub fn read_stamped_metadata(xba_path: &Path) -> std::io::Result<XbaMetadata> {
    let file = std::fs::File::open(xba_path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut entry = zip.by_name("metadata.json").map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "metadata.json not in xba")
    })?;
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf)?;
    serde_json::from_slice(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub fn read_entry_bytes(xba_path: &Path, entry_name: &str) -> std::io::Result<Vec<u8>> {
    let file = std::fs::File::open(xba_path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut entry = zip.by_name(entry_name).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} not in xba", entry_name),
        )
    })?;
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf)?;
    Ok(buf)
}

fn mime_of(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "image/jpeg",
    }
}

fn read_optional_entry(xba_path: &Path, name: Option<&str>) -> (Option<Vec<u8>>, Option<String>) {
    let Some(name) = name else {
        return (None, None);
    };
    match read_entry_bytes(xba_path, name) {
        Ok(bytes) if !bytes.is_empty() => (Some(bytes), Some(mime_of(name).to_string())),
        _ => (None, None),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Archive rewriter
// ─────────────────────────────────────────────────────────────────────────────

pub fn rewrite_xba_with_stamp(
    xba_path: &Path,
    stamped: &XbaMetadata,
    contributor_art: Option<(&str, &[u8])>,
    album_art: Option<(&str, &[u8])>,
) -> std::io::Result<()> {
    let parent = xba_path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "xba path has no parent")
    })?;
    let tmp = parent.join(format!(
        ".{}.stamp.tmp",
        xba_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("xba")
    ));

    let src = std::fs::File::open(xba_path)?;
    let mut src_zip = zip::ZipArchive::new(src)?;
    let dst = std::fs::File::create(&tmp)?;
    let mut dst_zip = zip::ZipWriter::new(dst);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);

    for i in 0..src_zip.len() {
        let mut entry = src_zip.by_index(i)?;
        let name = entry.name().to_string();
        if name == "metadata.json" {
            continue;
        }
        if name.starts_with("artwork_contributor.") {
            continue;
        }
        if name.starts_with("artwork_album.") {
            continue;
        }
        dst_zip.start_file(name, opts)?;
        std::io::copy(&mut entry, &mut dst_zip)?;
    }

    dst_zip.start_file("metadata.json", opts)?;
    dst_zip.write_all(&serde_json::to_vec_pretty(stamped)?)?;

    if let Some((name, bytes)) = contributor_art {
        dst_zip.start_file(name, opts)?;
        dst_zip.write_all(bytes)?;
    }
    if let Some((name, bytes)) = album_art {
        dst_zip.start_file(name, opts)?;
        dst_zip.write_all(bytes)?;
    }

    dst_zip.finish()?;
    std::fs::rename(&tmp, xba_path)?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Scan the hierarchy
// ─────────────────────────────────────────────────────────────────────────────

pub fn scan_installed_modules(root: &Path) -> Vec<RemoteAudioModuleInfo> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("xba") {
            continue;
        }
        let Ok(meta) = read_stamped_metadata(path) else {
            continue;
        };
        if meta.organization.stamped_at.is_empty() {
            continue;
        }
        out.push(remote_from_stamped(path, meta));
    }
    out
}

fn remote_from_stamped(path: &Path, meta: XbaMetadata) -> RemoteAudioModuleInfo {
    let (artwork_bytes, artwork_mime) =
        read_optional_entry(path, meta.stub.artwork_file.as_deref());
    let (contributor_artwork_bytes, contributor_artwork_mime) =
        read_optional_entry(path, meta.organization.contributor_artwork_entry.as_deref());
    let (album_artwork_bytes, album_artwork_mime) =
        read_optional_entry(path, meta.organization.album_artwork_entry.as_deref());
    let album_id = if meta.organization.album_id.is_empty() {
        None
    } else {
        Some(
            crate::engines::audio_engine::store::store_api_client::IdRef {
                id: meta.organization.album_id.clone(),
            },
        )
    };

    RemoteAudioModuleInfo {
        id: None,
        unique_id: meta.stub.unique_id,
        display_title: meta.stub.display_title,
        contributor: {
            let c = meta
                .organization
                .contributor_name
                .clone()
                .unwrap_or(meta.stub.contributor.clone());
            if c.is_empty() { None } else { Some(c) }
        },
        description: if meta.stub.description.is_empty() {
            None
        } else {
            Some(meta.stub.description)
        },
        duration_ms: meta.stub.duration_ms,
        features: crate::engines::audio_engine::store::store_api_client::HygraphFeatures {
            features: meta.stub.features,
        },
        source_url: meta.stub.source_url,
        artwork_file: None,
        album_id,
        version: meta.stub.version,
        created_at: String::new(),
        updated_at: String::new(),
        is_installed: true,
        status: ModuleStatus::Installed,
        artwork_bytes,
        artwork_mime,
        contributor_artwork_bytes,
        contributor_artwork_mime,
        album_artwork_bytes,
        album_artwork_mime,
    }
}
