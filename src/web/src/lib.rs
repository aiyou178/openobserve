// Copyright 2026 OpenObserve Inc.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! Keeps embedded UI assets isolated from API dependency rebuilds.

mod ui;

#[cfg(debug_assertions)]
use rust_embed_for_web::RustEmbed;
pub use ui::ui_routes;

const SOURCE_ARCHIVE: Option<(&str, &[u8])> =
    include!(concat!(env!("OUT_DIR"), "/source_archive.rs"));

#[cfg(not(debug_assertions))]
const WEB_ASSETS: &[(&str, rust_embed_for_web::EmbeddedFile)] =
    include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));

#[cfg(debug_assertions)]
#[derive(RustEmbed)]
#[folder = "../../web/dist/"]
#[exclude = "assets/openobserve-source-*.tar.gz"]
#[exclude = "assets/source-*.tar.gz"]
#[allow_missing = true]
#[gzip = false]
#[br = false]
pub struct WebAssets;

#[cfg(not(debug_assertions))]
pub struct WebAssets;

#[cfg(not(debug_assertions))]
impl WebAssets {
    fn get(path: &str) -> Option<rust_embed_for_web::EmbeddedFile> {
        WEB_ASSETS
            .binary_search_by_key(&path, |entry| entry.0)
            .ok()
            .map(|index| WEB_ASSETS[index].1)
    }
}

#[cfg(not(debug_assertions))]
impl rust_embed_for_web::RustEmbed for WebAssets {
    type File = rust_embed_for_web::EmbeddedFile;

    fn get(file_path: &str) -> Option<Self::File> {
        Self::get(file_path)
    }
}
