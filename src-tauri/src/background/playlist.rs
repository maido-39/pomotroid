//! The folder of images a backdrop cycles through.
//!
//! The folder is read where it sits rather than copied into the app: the user
//! keeps their pictures where they are, and adding or deleting one there
//! changes what the app shows without any bookkeeping. What that costs is
//! fragility — a moved folder or an unplugged drive empties the rotation —
//! which is the trade the "point at a folder" choice makes.
//!
//! The listing is taken once and kept. Re-reading a directory every round
//! change is a disk hit for a question whose answer rarely changes; it is
//! re-read when the folder setting changes, and whenever an entry has gone
//! missing underneath us.
//!
//! Order is a permutation the rotation walks, not a fresh random pick each
//! time. Picking at random repeats — with ten pictures, roughly every tenth
//! switch shows the one already on screen, which reads as a bug. Walking a
//! shuffled order shows all ten before any repeats, and the order is reshuffled
//! each time it runs out.

use std::path::{Path, PathBuf};

use super::{ALLOWED_EXTENSIONS, MAX_BYTES};

/// The rotation, shared as Tauri state.
pub type PlaylistState = std::sync::Arc<std::sync::Mutex<Playlist>>;

/// Upper bound on how many images one folder contributes. A photo library
/// pointed at by accident should not turn into an unbounded listing.
pub const MAX_ENTRIES: usize = 500;

/// Every image directly inside `dir`, in file-name order.
///
/// Sub-directories are not walked: "point at a folder" means the folder that
/// was pointed at. The order is by name so that turning shuffle off gives the
/// order the user already arranged — which is the whole point of naming files
/// `01-…`, `02-…`.
pub fn scan(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        // Size is checked here rather than when the picture is shown: a photo
        // too large to hand to the webview should never enter the rotation,
        // because skipping it later means a switch that changes nothing.
        .filter(|e| e.metadata().map(|m| m.len() <= MAX_BYTES as u64).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| is_image(p))
        .collect();
    out.sort_by_key(sort_key);
    out.truncate(MAX_ENTRIES);
    out
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|x| x.to_str())
        .map(|x| ALLOWED_EXTENSIONS.contains(&x.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn sort_key(path: &PathBuf) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// The order `len` images are walked in.
///
/// `avoid_first` is the image that was on screen when the previous order ran
/// out. Keeping it out of the first slot stops the rotation showing the same
/// picture twice across the seam, which is the one repeat a viewer notices.
pub fn build_order(len: usize, shuffle: bool, avoid_first: Option<usize>) -> Vec<usize> {
    crate::shuffle::permutation(len, shuffle, avoid_first)
}

/// The images a backdrop is cycling through, and where in them it is.
#[derive(Debug, Default)]
pub struct Playlist {
    folder: PathBuf,
    items: Vec<PathBuf>,
    /// Indices into `items`, in the order they are shown.
    order: Vec<usize>,
    cursor: usize,
    shuffle: bool,
}

impl Playlist {
    /// Point at `folder`, reading it now. Returns how many images it holds.
    pub fn load(&mut self, folder: &Path, shuffle: bool) -> usize {
        self.folder = folder.to_path_buf();
        self.items = scan(folder);
        self.shuffle = shuffle;
        self.order = build_order(self.items.len(), shuffle, None);
        self.cursor = 0;
        self.items.len()
    }

    /// Read the folder again, staying on the current image when it is still
    /// there. Used when an entry turns out to have gone.
    pub fn refresh(&mut self) -> usize {
        let showing = self.current();
        let folder = self.folder.clone();
        let shuffle = self.shuffle;
        self.load(&folder, shuffle);
        if let Some(showing) = showing {
            let at = self.order.iter().position(|&i| self.items[i] == showing);
            if let Some(at) = at {
                self.cursor = at;
            }
        }
        self.items.len()
    }

    /// Forget the folder; the backdrop falls back to the single stored image.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The image on screen.
    pub fn current(&self) -> Option<PathBuf> {
        let index = *self.order.get(self.cursor)?;
        self.items.get(index).cloned()
    }

    /// Move to the next image, reshuffling when the order runs out.
    pub fn advance(&mut self) -> Option<PathBuf> {
        if self.items.is_empty() {
            return None;
        }
        self.cursor += 1;
        if self.cursor >= self.order.len() {
            let finished = self.order.last().copied();
            self.order = build_order(self.items.len(), self.shuffle, finished);
            self.cursor = 0;
        }
        self.current()
    }

    /// Re-order without changing which folder is loaded.
    pub fn set_shuffle(&mut self, shuffle: bool) {
        if self.shuffle == shuffle {
            return;
        }
        let showing = self.current();
        self.shuffle = shuffle;
        self.order = build_order(self.items.len(), shuffle, None);
        let at = showing.and_then(|p| self.order.iter().position(|&i| self.items[i] == p));
        self.cursor = at.unwrap_or(0);
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pomotroid-playlist-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), b"x").unwrap();
    }

    #[test]
    fn scan_takes_images_in_name_order_and_nothing_else() {
        let dir = tmp("scan");
        touch(&dir, "02-beach.JPG");
        touch(&dir, "01-tower.png");
        touch(&dir, "notes.txt");
        touch(&dir, "archive.zip");
        std::fs::create_dir_all(dir.join("subfolder")).unwrap();
        touch(&dir.join("subfolder"), "buried.png");

        let found = scan(&dir);
        let names: Vec<String> =
            found.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["01-tower.png", "02-beach.JPG"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_leaves_out_a_file_too_large_to_show() {
        let dir = tmp("toobig");
        touch(&dir, "small.png");
        std::fs::write(dir.join("huge.png"), vec![0u8; MAX_BYTES + 1]).unwrap();

        let names: Vec<String> = scan(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["small.png"], "an oversized photo never enters the rotation");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_folder_is_empty_rather_than_an_error() {
        assert!(scan(Path::new("/definitely/not/here")).is_empty());
    }

    #[test]
    fn every_image_is_shown_before_any_repeats() {
        let dir = tmp("cycle");
        for n in 1..=5 {
            touch(&dir, &format!("{n}.png"));
        }
        let mut pl = Playlist::default();
        assert_eq!(pl.load(&dir, true), 5);

        let mut seen = vec![pl.current().unwrap()];
        for _ in 1..5 {
            seen.push(pl.advance().unwrap());
        }
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 5, "a full pass shows all five, none twice");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_single_image_folder_just_keeps_showing_it() {
        let dir = tmp("one");
        touch(&dir, "only.png");
        let mut pl = Playlist::default();
        pl.load(&dir, true);
        let only = pl.current().unwrap();
        assert_eq!(pl.advance().unwrap(), only);
        assert_eq!(pl.advance().unwrap(), only);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_folder_has_nothing_to_show() {
        let dir = tmp("empty");
        let mut pl = Playlist::default();
        assert_eq!(pl.load(&dir, true), 0);
        assert!(pl.current().is_none());
        assert!(pl.advance().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn switching_to_sorted_keeps_the_picture_on_screen() {
        let dir = tmp("shuffletoggle");
        for n in 1..=6 {
            touch(&dir, &format!("{n}.png"));
        }
        let mut pl = Playlist::default();
        pl.load(&dir, true);
        pl.advance();
        let showing = pl.current().unwrap();

        pl.set_shuffle(false);
        assert_eq!(pl.current().unwrap(), showing, "changing the order does not change the image");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_picks_up_a_new_file_without_losing_its_place() {
        let dir = tmp("refresh");
        for n in 1..=3 {
            touch(&dir, &format!("{n}.png"));
        }
        let mut pl = Playlist::default();
        pl.load(&dir, false);
        pl.advance();
        let showing = pl.current().unwrap();

        touch(&dir, "4.png");
        assert_eq!(pl.refresh(), 4);
        assert_eq!(pl.current().unwrap(), showing, "the picture on screen stays put");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
