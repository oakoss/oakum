//! Stable repository identity shared by CLI commands.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::Dir;

pub(super) struct Repository {
    path: PathBuf,
    dir: Dir,
}

impl Repository {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn dir(&self) -> &Dir {
        &self.dir
    }
}

pub(super) fn discover() -> Result<Repository, Box<dyn std::error::Error>> {
    let start_path = fs::canonicalize(std::env::current_dir()?)?;
    let start_dir = Dir::open_ambient_dir(&start_path, ambient_authority())?;
    let mut path = start_path.clone();
    let mut dir = start_dir.try_clone()?;
    loop {
        match dir.symlink_metadata(".git") {
            Ok(_) => return Ok(Repository { path, dir }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(Box::new(err)),
        }
        if !path.pop() {
            return Ok(Repository {
                path: start_path,
                dir: start_dir,
            });
        }
        dir = dir.open_parent_dir(ambient_authority())?;
    }
}
