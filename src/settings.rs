use serde::{Deserialize, Serialize};
use std::{fs, io::ErrorKind, path::PathBuf};

#[derive(Clone, Deserialize, Serialize)]
pub struct Settings {
    pub language: String,
    pub output_dir: PathBuf,
    pub fps: u32,
}

impl Settings {
    pub fn load_or_create(default_output_dir: PathBuf) -> Result<Self, String> {
        let path = settings_file();
        match fs::read_to_string(&path) {
            Ok(contents) => {
                let settings: Self = serde_json::from_str(&contents)
                    .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
                settings.validate()?;
                Ok(settings)
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let legacy_language = fs::read_to_string(language_file())
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                let settings = Self {
                    language: if legacy_language == "zh-TW" {
                        legacy_language
                    } else {
                        "en-US".to_owned()
                    },
                    output_dir: default_output_dir,
                    fps: 30,
                };
                settings.save()?;
                Ok(settings)
            }
            Err(error) => Err(format!("Could not read {}: {error}", path.display())),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        self.validate()?;
        let path = settings_file();
        let parent = path.parent().ok_or("Invalid settings path")?;
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let json = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        fs::write(&path, format!("{json}\n"))
            .map_err(|error| format!("Could not save {}: {error}", path.display()))
    }

    fn validate(&self) -> Result<(), String> {
        if self.language != "en-US" && self.language != "zh-TW" {
            return Err("Language must be en-US or zh-TW".to_owned());
        }
        if self.output_dir.as_os_str().is_empty() {
            return Err("Output folder cannot be empty".to_owned());
        }
        if !(10..=60).contains(&self.fps) {
            return Err("Frame rate must be between 10 and 60 FPS".to_owned());
        }
        Ok(())
    }
}

pub fn settings_file() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("RecordScreen")
        .join("setting.json")
}

fn language_file() -> PathBuf {
    settings_file().with_file_name("language.txt")
}
