use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Env vars to point at data shipped next to the executable, in a portable
/// layout `<exe dir>/share/{gdal,proj}`. Variables the user already set win.
fn bundled_data_env(
    exe_dir: &Path,
    get: impl Fn(&str) -> Option<OsString>,
) -> Vec<(&'static str, PathBuf)> {
    [("GDAL_DATA", "gdal"), ("PROJ_DATA", "proj")]
        .into_iter()
        .filter(|(var, _)| get(var).is_none())
        .map(|(var, sub)| (var, exe_dir.join("share").join(sub)))
        .filter(|(_, dir)| dir.is_dir())
        .collect()
}

/// Points GDAL and PROJ at `<exe dir>/share/{gdal,proj}` when those folders
/// exist, so a portable install finds its own data files. Call first in
/// `main`, before any thread starts (it mutates the process environment).
pub fn use_bundled_data() {
    let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    else {
        return;
    };
    for (var, dir) in bundled_data_env(&exe_dir, |k| std::env::var_os(k)) {
        std::env::set_var(var, dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_dirs_set_only_when_present_and_unset() {
        let dir = std::env::temp_dir().join(format!("snap-bundle-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("share/gdal")).unwrap();
        std::fs::create_dir_all(dir.join("share/proj")).unwrap();

        let none = |_: &str| None;
        assert_eq!(
            bundled_data_env(&dir, none),
            vec![
                ("GDAL_DATA", dir.join("share/gdal")),
                ("PROJ_DATA", dir.join("share/proj")),
            ]
        );

        let user_set = |k: &str| (k == "PROJ_DATA").then(|| OsString::from("/custom"));
        assert_eq!(
            bundled_data_env(&dir, user_set),
            vec![("GDAL_DATA", dir.join("share/gdal"))]
        );

        std::fs::remove_dir_all(dir.join("share/proj")).unwrap();
        std::fs::remove_dir_all(dir.join("share/gdal")).unwrap();
        assert!(bundled_data_env(&dir, none).is_empty());
        std::fs::remove_dir_all(dir).ok();
    }
}
