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

use std::{
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use rust_embed_for_web::{DynamicFile, EmbedableFile, utils::Config};

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    println!("cargo:rerun-if-changed={manifest_dir}/src/lib.rs");

    // E2E workflows rewrite the embed folder outside the repository.
    let lib = fs::read_to_string(format!("{manifest_dir}/src/lib.rs"))
        .expect("failed to read src/lib.rs");
    let folder = lib
        .split("#[folder = \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("no #[folder = \"...\"] attribute found in src/lib.rs");
    let dist = Path::new(manifest_dir).join(folder);

    // The derive does not track files, so rebuilt assets must invalidate this crate.
    println!("cargo:rerun-if-changed={}", dist.display());
    println!("cargo:rerun-if-env-changed=OPENOBSERVE_SOURCE_ASSET");

    // Missing assets otherwise silently produce an empty UI at runtime.
    if !dist.join("index.html").is_file() {
        println!(
            "cargo:warning={} does not contain index.html; the binary will serve an empty web UI",
            dist.display()
        );
    }

    let expected = env::var_os("OPENOBSERVE_SOURCE_ASSET").map(|asset| {
        asset
            .into_string()
            .expect("OPENOBSERVE_SOURCE_ASSET is not UTF-8")
    });
    let expression = match source_archive(&dist, expected.as_deref()) {
        Some((asset, file)) => {
            println!("cargo:rerun-if-changed={}", file.display());
            format!(
                "Some(({asset:?}, include_bytes!({:?})))",
                file.to_str().unwrap()
            )
        }
        None => "None".to_string(),
    };
    let out_dir = env::var_os("OUT_DIR").expect("OUT_DIR is not set");
    fs::write(Path::new(&out_dir).join("source_archive.rs"), expression)
        .expect("failed to write embedded source archive mapping");
    fs::write(
        Path::new(&out_dir).join("web_assets.rs"),
        embedded_assets(&dist),
    )
    .expect("failed to write embedded web asset table");
}

fn embedded_assets(dist: &Path) -> String {
    let mut config = Config::new();
    config.set_allow_missing(true);
    config.set_gzip(false);
    config.set_br(false);
    config.set_zstd(false);
    config.add_exclude("assets/openobserve-source-*.tar.gz".to_string());
    config.add_exclude("assets/source-*.tar.gz".to_string());
    let mut entries: Vec<_> = rust_embed_for_web::utils::get_files(
        dist.to_str().expect("web asset directory is not UTF-8"),
        &config,
        "",
    )
    .collect();
    entries.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));

    let mut table = String::from("&[\n");
    for entry in entries {
        println!("cargo:rerun-if-changed={}", entry.full_canonical_path);
        let file = DynamicFile::read_from_fs(&entry.full_canonical_path)
            .expect("failed to read embedded web asset");
        // include_bytes avoids expanding each byte into a Rust AST expression.
        table.push_str(&format!(
            "({:?}, rust_embed_for_web::EmbeddedFile::__internal_make(\
             {:?}, include_bytes!({:?}), None, None, None, {:?}, {:?}, {:?}, {:?}, {:?})),\n",
            entry.rel_path,
            file.name(),
            entry.full_canonical_path,
            file.hash(),
            file.etag(),
            file.last_modified(),
            file.last_modified_timestamp(),
            file.mime_type(),
        ));
    }
    table.push(']');
    table
}

fn source_archive(dist: &Path, expected: Option<&str>) -> Option<(String, PathBuf)> {
    let mut archives = Vec::new();
    match fs::read_dir(dist.join("assets")) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.expect("failed to read web asset entry");
                let name = entry
                    .file_name()
                    .into_string()
                    .expect("web asset filename is not UTF-8");
                if (name.starts_with("openobserve-source-") || name.starts_with("source-"))
                    && name.ends_with(".tar.gz")
                {
                    assert!(
                        entry.file_type().unwrap().is_file(),
                        "source archive must be a regular file: {name}"
                    );
                    archives.push((format!("assets/{name}"), entry.path()));
                }
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => panic!("failed to read web assets: {error}"),
    }
    assert!(
        archives.len() <= 1,
        "multiple corresponding source archives found in web assets"
    );
    let archive = archives.pop();
    if let Some(expected) = expected {
        assert_eq!(
            archive.as_ref().map(|(asset, _)| asset.as_str()),
            Some(expected),
            "OPENOBSERVE_SOURCE_ASSET must match the sole corresponding source archive"
        );
    }
    archive.map(|(asset, path)| {
        (
            asset,
            path.canonicalize()
                .expect("failed to resolve corresponding source archive"),
        )
    })
}
