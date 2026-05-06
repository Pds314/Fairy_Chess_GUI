// src/asset_manager.rs
use std::fs;
use std::path::{Path, PathBuf};

/// Represents a browser item that can be either a file or directory
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserItem {
    File(PathBuf),
    Directory(PathBuf),
    UpDirectory,
    ToAssets,
}

impl std::fmt::Display for BrowserItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrowserItem::File(path) => write!(
                f,
                "{}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
            BrowserItem::Directory(path) => write!(
                f,
                "{}/",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
            BrowserItem::UpDirectory => write!(f, "../"),
            BrowserItem::ToAssets => write!(f, "⌂ Back to Assets"),
        }
    }
}

/// Manages asset paths and discovery for the application
#[derive(Debug)]
pub struct AssetManager {
    asset_root: Option<PathBuf>,
    current_browse_dir: Option<PathBuf>,
}

impl AssetManager {
    pub fn new() -> Self {
        let asset_root = Self::find_assets_directory();
        Self {
            current_browse_dir: asset_root.clone(),
            asset_root,
        }
    }

    /// Find the assets directory by searching up the directory tree
    fn find_assets_directory() -> Option<PathBuf> {
        // Try multiple starting points in order of preference
        let starting_points = vec![
            std::env::current_dir().ok(),
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.to_path_buf())),
            std::env::var("CARGO_MANIFEST_DIR").ok().map(PathBuf::from),
        ];

        for start_point in starting_points.into_iter().flatten() {
            if let Some(found) = Self::search_upward_for_assets(&start_point) {
                println!("Found assets directory at: {}", found.display());
                return Some(found);
            }
        }

        println!("Warning: Could not find assets directory");
        None
    }

    /// Search upward from a given path to find the assets directory
    fn search_upward_for_assets(start_path: &Path) -> Option<PathBuf> {
        let mut current = start_path.to_path_buf();

        // First check if start_path itself contains assets
        let assets_path = current.join("assets");
        if Self::is_valid_assets_dir(&assets_path) {
            return Some(assets_path);
        }

        // Then search upward up to 5 levels
        for _ in 0..5 {
            if let Some(parent) = current.parent() {
                let assets_path = parent.join("assets");
                if Self::is_valid_assets_dir(&assets_path) {
                    return Some(assets_path);
                }
                current = parent.to_path_buf();
            } else {
                break;
            }
        }

        None
    }

    /// Check if a path is a valid assets directory
    fn is_valid_assets_dir(path: &Path) -> bool {
        path.exists() && path.is_dir()
    }

    /// Get the path to a specific asset file
    pub fn get_asset_path(&self, relative_path: &str) -> Option<PathBuf> {
        self.asset_root
            .as_ref()
            .map(|root| root.join(relative_path))
    }

    /// Get the pieces config file path (legacy support)
    pub fn get_pieces_config_path(&self) -> Option<PathBuf> {
        // Try new extension first, then fallback to legacy
        if let Some(path) = self.get_asset_path("FIDE.pieces") {
            if path.exists() {
                return Some(path);
            }
        }
        self.get_asset_path("pieces.config")
    }

    /// Get the game config file path (for board setup) - legacy support
    pub fn get_game_config_path(&self) -> Option<PathBuf> {
        // Try FIDE.game first (our default), then try other possible names
        let config_names = ["FIDE.game", "game.config", "board.config", "setup.config"];

        for name in &config_names {
            if let Some(path) = self.get_asset_path(name) {
                if path.exists() {
                    return Some(path);
                }
            }
        }

        None
    }

    /// Get a specific game file path
    pub fn get_game_file_path(&self, filename: &str) -> Option<PathBuf> {
        if filename.is_empty() {
            return self.get_game_config_path();
        }

        // If it's an absolute path within our browsing context
        let path = Path::new(filename);
        if path.is_absolute() {
            if path.exists() && path.extension().and_then(|s| s.to_str()) == Some("game") {
                return Some(path.to_path_buf());
            }
        } else if let Some(current_dir) = &self.current_browse_dir {
            let full_path = current_dir.join(filename);
            if full_path.exists() {
                return Some(full_path);
            }
        }

        // Fallback to asset path
        self.get_asset_path(filename)
    }

    /// Get a specific pieces file path
    pub fn get_pieces_file_path(&self, filename: &str) -> Option<PathBuf> {
        if filename.is_empty() {
            return self.get_pieces_config_path();
        }

        // If it's an absolute path within our browsing context
        let path = Path::new(filename);
        if path.is_absolute() {
            if path.exists() && path.extension().and_then(|s| s.to_str()) == Some("pieces") {
                return Some(path.to_path_buf());
            }
        } else if let Some(current_dir) = &self.current_browse_dir {
            let full_path = current_dir.join(filename);
            if full_path.exists() {
                return Some(full_path);
            }
        }

        // Fallback to asset path
        self.get_asset_path(filename)
    }

    /// Get the pieces directory path
    pub fn get_pieces_directory(&self) -> Option<PathBuf> {
        self.get_asset_path("pieces")
    }

    /// Directory where `.personality` files live. May not exist; caller
    /// handles that gracefully.
    pub fn get_personalities_directory(&self) -> Option<PathBuf> {
        self.get_asset_path("personalities")
    }

    /// Check if a specific piece texture exists
    pub fn piece_texture_exists(&self, filename: &str) -> bool {
        if let Some(pieces_dir) = self.get_pieces_directory() {
            pieces_dir.join(filename).exists()
        } else {
            false
        }
    }

    /// List all available piece textures
    pub fn list_piece_textures(&self) -> Vec<String> {
        if let Some(pieces_dir) = self.get_pieces_directory() {
            if let Ok(entries) = fs::read_dir(pieces_dir) {
                return entries
                    .filter_map(|entry| {
                        entry.ok().and_then(|e| {
                            let path = e.path();
                            if path.extension()?.to_str()? == "png" {
                                path.file_name()?.to_str().map(String::from)
                            } else {
                                None
                            }
                        })
                    })
                    .collect();
            }
        }
        Vec::new()
    }

    pub fn has_assets(&self) -> bool {
        self.asset_root.is_some()
    }

    /// Get the current browse directory
    pub fn get_current_browse_dir(&self) -> Option<&PathBuf> {
        self.current_browse_dir.as_ref()
    }

    /// Set the current browse directory
    pub fn set_current_browse_dir(&mut self, path: PathBuf) {
        println!("Browsing to directory: {}", path.display());
        self.current_browse_dir = Some(path);
    }

    /// Navigate to the parent directory
    pub fn navigate_up(&mut self) -> bool {
        if let Some(current) = &self.current_browse_dir {
            if let Some(parent) = current.parent() {
                println!(
                    "Navigating up from {} to {}",
                    current.display(),
                    parent.display()
                );
                self.current_browse_dir = Some(parent.to_path_buf());
                return true;
            }
        }
        false
    }

    /// Return to the assets root directory
    pub fn navigate_to_assets(&mut self) -> bool {
        if let Some(assets_root) = &self.asset_root {
            println!("Returning to assets directory: {}", assets_root.display());
            self.current_browse_dir = Some(assets_root.clone());
            return true;
        }
        false
    }

    /// List game files in the current browse directory
    pub fn list_game_files(&self) -> Vec<BrowserItem> {
        self.list_files_with_extension("game")
    }

    /// List pieces files in the current browse directory
    pub fn list_pieces_files(&self) -> Vec<BrowserItem> {
        self.list_files_with_extension("pieces")
    }

    /// List files with a specific extension, plus navigation options
    fn list_files_with_extension(&self, extension: &str) -> Vec<BrowserItem> {
        let mut items = Vec::new();

        let current_dir = match &self.current_browse_dir {
            Some(dir) => dir,
            None => return items,
        };

        println!(
            "Listing {} files in directory: {}",
            extension,
            current_dir.display()
        );

        // Add navigation options
        if let Some(asset_root) = &self.asset_root {
            // Only add "up" option if we're not already at or above the asset root
            if current_dir != asset_root && current_dir.starts_with(asset_root) {
                items.push(BrowserItem::UpDirectory);
            }

            // Always add "back to assets" option unless we're already there
            if current_dir != asset_root {
                items.push(BrowserItem::ToAssets);
            }
        }

        // Read directory contents
        if let Ok(entries) = fs::read_dir(current_dir) {
            let mut directories = Vec::new();
            let mut files = Vec::new();

            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();

                if path.is_dir() {
                    directories.push(BrowserItem::Directory(path));
                } else if path.extension().and_then(|s| s.to_str()) == Some(extension) {
                    files.push(BrowserItem::File(path));
                }
            }

            // Sort directories and files separately
            directories.sort_by(|a, b| {
                if let (BrowserItem::Directory(a), BrowserItem::Directory(b)) = (a, b) {
                    a.file_name().cmp(&b.file_name())
                } else {
                    std::cmp::Ordering::Equal
                }
            });

            files.sort_by(|a, b| {
                if let (BrowserItem::File(a), BrowserItem::File(b)) = (a, b) {
                    a.file_name().cmp(&b.file_name())
                } else {
                    std::cmp::Ordering::Equal
                }
            });

            // Add directories first, then files
            items.extend(directories);
            items.extend(files);
        }

        println!(
            "Found {} items ({} {} files)",
            items.len(),
            items
                .iter()
                .filter(|item| matches!(item, BrowserItem::File(_)))
                .count(),
            extension
        );

        items
    }

    /// Handle browser item selection
    pub fn handle_browser_selection(&mut self, item: &BrowserItem) -> Option<PathBuf> {
        match item {
            BrowserItem::File(path) => {
                println!("Selected file: {}", path.display());
                Some(path.clone())
            }
            BrowserItem::Directory(path) => {
                self.set_current_browse_dir(path.clone());
                None
            }
            BrowserItem::UpDirectory => {
                self.navigate_up();
                None
            }
            BrowserItem::ToAssets => {
                self.navigate_to_assets();
                None
            }
        }
    }
}

impl Default for AssetManager {
    fn default() -> Self {
        Self::new()
    }
}
