//  artwork.rs
//  XBible Core Engine
//
use std::{fs::File, io::{Cursor, Read}, path::Path};
use image::{GenericImageView, ImageReader};
use crate::engines::audio_engine::{engine::ModuleMetadata, utils::color::AudioEngineRGBAColor};

#[derive(Debug, Clone, uniffi::Object)]
pub struct Artwork {
    pub image_bytes: Option<Vec<u8>>,
}

#[uniffi::export]
impl Artwork {
    #[uniffi::constructor]
    pub fn new(file_path: String) -> Self {
        let mut image_bytes: Option<Vec<u8>> = None;

        // --- EXTRACT ARTWORK BYTES DIRECTLY INSIDE CONSTRUCTOR ---
        if let Some(path) = Some(Path::new(&file_path)) {
            if let Ok(file) = File::open(path) {
                if let Ok(mut archive) = zip::ZipArchive::new(file) {
                    
                    let mut target_artwork_name: Option<String> = None;

                    // Scope block to safely drop the metadata file borrow early
                    {
                        if let Ok(mut meta_file) = archive.by_name("metadata.json") {
                            let mut contents = String::new();
                            if meta_file.read_to_string(&mut contents).is_ok() {
                                if let Ok(metadata) = serde_json::from_str::<ModuleMetadata>(&contents) {
                                    target_artwork_name = metadata.artwork_file;
                                }
                            }
                        }
                    }

                    // Strategy 1: Read from metadata targets
                    if let Some(artwork_name) = target_artwork_name {
                        if let Ok(mut image_file) = archive.by_name(&artwork_name) {
                            let mut buffer = Vec::new();
                            if image_file.read_to_end(&mut buffer).is_ok() {
                                image_bytes = Some(buffer);
                            }
                        }
                    }

                    // Strategy 2: Common Fallbacks
                    if image_bytes.is_none() {
                        let common_fallbacks = [
                            "artwork.jpg", "artwork.jpeg", "artwork.png",
                            "cover.jpg", "cover.jpeg", "cover.png",
                            "image.jpg", "image.png",
                        ];

                        for filename in common_fallbacks.iter() {
                            if let Ok(mut image_file) = archive.by_name(filename) {
                                let mut buffer = Vec::new();
                                if image_file.read_to_end(&mut buffer).is_ok() {
                                    image_bytes = Some(buffer);
                                    break;
                                }
                            }
                        }
                    }

                    // Strategy 3: Grab the first image format inside as last resort
                    if image_bytes.is_none() {
                        for i in 0..archive.len() {
                            if let Ok(mut file) = archive.by_index(i) {
                                let name = file.name().to_lowercase();
                                if name.ends_with(".jpg")
                                    || name.ends_with(".jpeg")
                                    || name.ends_with(".png")
                                    || name.ends_with(".webp")
                                {
                                    let mut buffer = Vec::new();
                                    if file.read_to_end(&mut buffer).is_ok() {
                                        image_bytes = Some(buffer);
                                        break;
                                    }
                                }
                            }
                        }
                    }

                }
            }
        }

        Self { image_bytes }
    }

    pub fn image_bytes(&self) -> Option<Vec<u8>> {
        self.image_bytes.clone()
    }

    /// Extracts colors directly targeting the internal image_bytes payload option
    pub fn extract_colors(&self, count: u32) -> Vec<AudioEngineRGBAColor> {
        let Some(ref bytes) = self.image_bytes else {
            return Vec::new();
        };

        // Guess format from internal reader stream bytes
        let Ok(reader) = ImageReader::new(Cursor::new(bytes)).with_guessed_format() else {
            return Vec::new();
        };

        // Decode the raw vector stream into memory
        let Ok(img) = reader.decode() else {
            return Vec::new();
        };

        let (width, height) = img.dimensions();
        if height == 0 || width == 0 || count == 0 {
            return Vec::new();
        }
        
        let tile_height = height / count;
        let mut extracted_colors = Vec::new();

        // Squeeze through the tiles to extract average spectrum positions
        for i in 0..count {
            let start_y = i * tile_height;
            let end_y = if i == count - 1 {
                height
            } else {
                start_y + tile_height
            };

            let mut total_r: u64 = 0;
            let mut total_g: u64 = 0;
            let mut total_b: u64 = 0;
            let mut total_a: u64 = 0;
            let mut pixel_count: u64 = 0;

            for y in start_y..end_y {
                for x in 0..width {
                    let pixel = img.get_pixel(x, y);
                    total_r += pixel[0] as u64;
                    total_g += pixel[1] as u64;
                    total_b += pixel[2] as u64;
                    total_a += pixel[3] as u64;
                    pixel_count += 1;
                }
            }

            if pixel_count > 0 {
                extracted_colors.push(AudioEngineRGBAColor {
                    red: (total_r as f64 / pixel_count as f64) / 255.0,
                    green: (total_g as f64 / pixel_count as f64) / 255.0,
                    blue: (total_b as f64 / pixel_count as f64) / 255.0,
                    alpha: (total_a as f64 / pixel_count as f64) / 255.0,
                });
            }
        }

        extracted_colors
    }
}