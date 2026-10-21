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

use std::{env, fs, io::Result, path::Path, process::Command};

use chrono::{DateTime, SecondsFormat, Utc};

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=OPENOBSERVE_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=OPENOBSERVE_BUILD_COMMIT");
    let version_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contrib/aiyou/VERSION");
    println!("cargo:rerun-if-changed={}", version_file.display());

    for git_path in ["HEAD", "packed-refs"] {
        if let Some(path) = git_value(&["rev-parse", "--git-path", git_path]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(reference) = git_value(&["symbolic-ref", "--quiet", "HEAD"])
        && let Some(path) = git_value(&["rev-parse", "--git-path", &reference])
    {
        println!("cargo:rerun-if-changed={path}");
    }

    let version = env::var("OPENOBSERVE_BUILD_VERSION")
        .ok()
        .or_else(|| fs::read_to_string(version_file).ok())
        .or_else(|| git_value(&["describe", "--tags", "--abbrev=0"]))
        .unwrap_or_else(|| "unknown".to_string());
    let git_commit = env::var("OPENOBSERVE_BUILD_COMMIT")
        .ok()
        .or_else(|| git_value(&["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    let version = version.trim();
    let git_commit = git_commit.trim();
    assert!(!version.is_empty() && !version.contains(['\n', '\r']));
    assert!(!git_commit.is_empty() && !git_commit.contains(['\n', '\r']));
    println!("cargo:rustc-env=GIT_VERSION={version}");
    println!("cargo:rustc-env=GIT_COMMIT_HASH={git_commit}");

    let now: DateTime<Utc> = Utc::now();
    let build_date = now.to_rfc3339_opts(SecondsFormat::Secs, true);
    println!("cargo:rustc-env=GIT_BUILD_DATE={build_date}");

    Ok(())
}

fn git_value(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}
