use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize, Serialize)]
pub struct Settings {
    pub language: String,
    pub output_dir: PathBuf,
    pub fps: u32,
    #[serde(default)]
    pub countdown_seconds: u32,
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
                    countdown_seconds: 0,
                };
                settings.save()?;
                Ok(settings)
            }
            Err(error) => Err(format!("Could not read {}: {error}", path.display())),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&settings_file())
    }

    fn save_to(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        let parent = path.parent().ok_or("Invalid settings path")?;
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let json = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        // Write beside the destination so the final replacement stays on one filesystem.
        let save = || -> Result<(), std::io::Error> {
            let mut pending = tempfile::NamedTempFile::new_in(parent)?;
            writeln!(pending, "{json}")?;
            pending.as_file().sync_all()?;
            pending.persist(path).map_err(|error| error.error)?;
            Ok(())
        };
        save().map_err(|error| format!("Could not save {}: {error}", path.display()))
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
        if ![0, 3, 5, 10].contains(&self.countdown_seconds) {
            return Err("Countdown must be 0, 3, 5, or 10 seconds".to_owned());
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

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn old_settings_keep_immediate_recording_default() {
        let old = r#"{"language":"en-US","output_dir":"C:\\RecordScreen","fps":30}"#;
        let settings: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(settings.countdown_seconds, 0);
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn unsupported_countdown_is_rejected() {
        let settings = Settings {
            language: "en-US".to_owned(),
            output_dir: "C:\\RecordScreen".into(),
            fps: 30,
            countdown_seconds: 7,
        };
        assert!(settings.validate().is_err());
    }

    #[test]
    fn saved_settings_replace_existing_json_without_temporary_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("setting.json");
        std::fs::write(&path, "previous settings").unwrap();
        let settings = Settings {
            language: "zh-TW".into(),
            output_dir: dir.path().join("captures"),
            fps: 60,
            countdown_seconds: 5,
        };
        settings.save_to(&path).unwrap();
        let saved: Settings = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.language, "zh-TW");
        assert_eq!(saved.fps, 60);
        assert_eq!(saved.countdown_seconds, 5);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn failed_replacement_preserves_previous_settings_and_cleans_up() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("setting.json");
        std::fs::write(&path, "original settings").unwrap();
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let settings = Settings {
            language: "en-US".into(),
            output_dir: dir.path().join("captures"),
            fps: 30,
            countdown_seconds: 0,
        };
        assert!(settings.save_to(&path).is_err());
        drop(locked);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original settings");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
