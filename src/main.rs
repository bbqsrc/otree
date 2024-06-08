use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
};

use clap::Parser;

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long, env)]
    codesign_identity: String,
    #[arg(short, long, default_value = "arm64")]
    arch: String,
    root_path: String,
    #[arg(short, long)]
    output_path: PathBuf,
}

fn main() {
    let args = Args::parse();
    println!("{:#?}", args);

    let resolver = Resolver::new(args.root_path);

    let deps = resolver.collect_deps();

    let required = deps
        .values()
        .flat_map(|x| x.values())
        .filter_map(|(res, path)| {
            if matches!(res, Resolution::Required) {
                Some(path)
            } else {
                None
            }
        })
        .collect::<BTreeSet<_>>();

    let sysroot = resolver.sysroot_dylibs();

    println!("{:?}", sysroot.len());

    std::fs::create_dir_all(&args.output_path).unwrap();

    for path in required {
        println!("{}", path.display());
        let target_path = args.output_path.join(path.file_name().unwrap());
        let _ = std::fs::remove_file(&target_path);
        std::fs::copy(&path, &target_path).unwrap();

        std::process::Command::new("install_name_tool")
            .arg("-id")
            .arg(format!(
                "@rpath/{}",
                path.file_name().unwrap().to_str().unwrap()
            ))
            .arg(&target_path)
            .output()
            .unwrap();

        for (dep_name, (res, dest)) in deps.get(path).unwrap().iter() {
            if matches!(res, Resolution::Required) {
                std::process::Command::new("install_name_tool")
                    .arg("-change")
                    .arg(&dep_name)
                    .arg(format!(
                        "@rpath/{}",
                        dest.file_name().unwrap().to_str().unwrap()
                    ))
                    .arg(&target_path)
                    .output()
                    .unwrap();
            }
        }
        let output = std::process::Command::new("codesign")
            .arg("-s")
            .arg(&args.codesign_identity)
            .arg("-f")
            .arg(&target_path)
            .output()
            .unwrap();
        let err = std::str::from_utf8(&output.stderr).unwrap();
        // let output = std::str::from_utf8(&output.stdout).unwrap();
        println!("{}", err);
    }

    for path in sysroot {
        println!("{}", path.display());
        for (dep_name, (res, dest)) in deps.get(path).unwrap().iter() {
            if matches!(res, Resolution::Required) {
                std::process::Command::new("install_name_tool")
                    .arg("-change")
                    .arg(&dep_name)
                    .arg(format!(
                        "@rpath/{}",
                        dest.file_name().unwrap().to_str().unwrap()
                    ))
                    .arg(&path)
                    .output()
                    .unwrap();
            }
        }
        let output = std::process::Command::new("codesign")
            .arg("-s")
            .arg(&args.codesign_identity)
            .arg("-f")
            .arg(&path)
            .output()
            .unwrap();
        let err = std::str::from_utf8(&output.stderr).unwrap();
        println!("{}", err);
    }

    // for (abspath, blah) in deps {
    //     println!("{}", abspath.display());
    //     for (depname, (res, deppath)) in blah.iter() {
    //         println!("    {depname} -> {res:?} ({})", deppath.display());
    //     }
    //     println!();
    // }
}

fn find_all_sysroot_dylibs(path: &str) -> Vec<PathBuf> {
    let output = std::process::Command::new("find")
        .args([&path, "-name", "*.dylib"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let other_output = std::process::Command::new("find")
        .args([&path, "-name", "*.so"])
        .output()
        .unwrap();
    let other_stdout = String::from_utf8(other_output.stdout).unwrap();
    stdout
        .lines()
        .chain(other_stdout.lines())
        .map(PathBuf::from)
        .collect::<Vec<_>>()
}

fn find_all_homebrew_dylibs() -> Vec<PathBuf> {
    let output = std::process::Command::new("find")
        .args(["/opt/homebrew/Cellar", "-name", "*.dylib"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    stdout.lines().map(PathBuf::from).collect::<Vec<_>>()
}

fn find_all_usr_lib_dylibs() -> Vec<PathBuf> {
    let output = std::process::Command::new("find")
        .args(["/usr/lib", "-name", "*.dylib"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    stdout.lines().map(PathBuf::from).collect::<Vec<_>>()
}

struct Resolver {
    homebrew_dylibs: Vec<PathBuf>,
    homebrew_lookup: HashMap<String, PathBuf>,
    usr_lib_dylibs: Vec<PathBuf>,
    usr_lib_lookup: HashMap<String, PathBuf>,
    sysroot: String,
    sysroot_dylibs: Vec<PathBuf>,
    sysroot_lookup: HashMap<String, PathBuf>,
}

#[derive(Debug, Clone, Copy)]
enum Resolution {
    /// No action needs to be taken, provided by system
    System,
    /// Must be cloned and mangled for @rpath support
    Required,
    /// Is requested from non-system source but does not seem to be present on the system
    Missing,
    /// Is included in the sysroot, so we shouldn't need to do anything other than mangle maybe
    Sysroot,
    /// Some weird state has occurred, we don't know.
    Unknown,
}

fn dyld_info(path: impl AsRef<Path>, arch: &str) -> Option<Vec<String>> {
    let path = path.as_ref();

    let output = std::process::Command::new("dyld_info")
        .arg("-arch")
        .arg(arch)
        .arg("-dependents")
        .arg(path)
        .output()
        .unwrap();
    let err = std::str::from_utf8(&output.stderr).unwrap();
    let output = std::str::from_utf8(&output.stdout).unwrap();

    if err.trim().ends_with("file not found") {
        return None;
    }

    let output = output
        .trim()
        .split("attributes     load path")
        .skip(1)
        .next()
        .unwrap();

    let output = output
        .trim()
        .split('\n')
        .filter_map(|x| {
            x.trim()
                .split_ascii_whitespace()
                .last()
                .map(|x| x.to_string())
        })
        .collect::<Vec<String>>();

    // println!("{:?}", output);
    for o in output.iter() {
        if o == "-dependents:" {
            panic!("{path:?}");
        }
    }

    Some(output)
}

impl Resolver {
    pub fn new(sysroot: impl Into<String>) -> Self {
        let sysroot = sysroot.into();
        let sysroot_dylibs = find_all_sysroot_dylibs(&sysroot);
        println!("{:#?}", &sysroot_dylibs);
        let sysroot_lookup = sysroot_dylibs
            .iter()
            .map(|x| {
                let file_name = x.file_name().unwrap().to_string_lossy().to_string();
                (file_name, x.clone())
            })
            .collect::<HashMap<_, _>>();

        let homebrew_dylibs = find_all_homebrew_dylibs();
        let homebrew_lookup = homebrew_dylibs
            .iter()
            .map(|x| {
                let file_name = x.file_name().unwrap().to_string_lossy().to_string();
                (file_name, x.clone())
            })
            .collect::<HashMap<_, _>>();

        let usr_lib_dylibs = find_all_usr_lib_dylibs();
        let usr_lib_lookup = usr_lib_dylibs
            .iter()
            .map(|x| {
                let file_name = x.file_name().unwrap().to_string_lossy().to_string();
                (file_name, x.clone())
            })
            .collect::<HashMap<_, _>>();

        Self {
            homebrew_lookup,
            homebrew_dylibs,
            usr_lib_dylibs,
            usr_lib_lookup,
            sysroot,
            sysroot_dylibs,
            sysroot_lookup,
        }
    }

    fn sysroot_dylibs(&self) -> &[PathBuf] {
        &self.sysroot_dylibs
    }

    fn collect_deps(&self) -> HashMap<PathBuf, HashMap<String, (Resolution, PathBuf)>> {
        let mut args = self
            .sysroot_dylibs
            .iter()
            .map(|x| x.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        // let mut seen: HashMap<PathBuf, Vec<String>> = HashMap::new();
        let mut out: HashMap<PathBuf, HashMap<String, (Resolution, PathBuf)>> = HashMap::new();

        loop {
            let mut current = vec![];
            std::mem::swap(&mut current, &mut args);

            if current.is_empty() {
                break;
            }

            for base_arg in current {
                let (resolution, resolved_path) = self.resolve_rpath(&base_arg);

                match resolution {
                    Resolution::System => {
                        // println!("Is system, continuing.");
                        continue;
                    }
                    Resolution::Required => {
                        if out.contains_key(&resolved_path) {
                            // println!("Is found in set, continuing.");
                            continue;
                        }

                        // print!("[{base_arg}] Is required: ");
                    }
                    Resolution::Missing => {
                        println!("[{base_arg}] Is missing, continuing.");
                        continue;
                    }
                    Resolution::Sysroot => {
                        if out.contains_key(&resolved_path) {
                            // println!("Is found in set, continuing.");
                            continue;
                        }

                        // print!("[{base_arg}] Is sysroot: ");
                    }
                    Resolution::Unknown => {
                        println!("[{base_arg}] Is unknown, continuing.");
                        continue;
                    }
                };

                // println!("Processing: {base_arg}");

                let Some(output) = dyld_info(&resolved_path, "arm64") else {
                    panic!("OH NO");
                };

                args.extend_from_slice(&output);
                out.insert(
                    resolved_path,
                    output
                        .iter()
                        .map(|x| (x.to_string(), self.resolve_rpath(x)))
                        .collect(),
                );
            }
        }

        out
    }

    pub fn resolve_rpath(&self, file_path: &str) -> (Resolution, PathBuf) {
        // So this is where things get fun. We need to check the OS first, and then in the Homebrew pile of libs

        if file_path.starts_with(&self.sysroot) {
            return (Resolution::Sysroot, PathBuf::from(file_path));
        }

        let is_absolute = file_path.starts_with("/");
        let is_special = file_path.starts_with("@");

        if is_absolute {
            if ["/System", "/Library", "/usr/lib"]
                .iter()
                .any(|x| file_path.starts_with(x))
            {
                return (Resolution::System, PathBuf::from(file_path));
            }

            if file_path.starts_with("/opt/homebrew") {
                let file_name = file_path.rsplit("/").next().unwrap();
                if let Some(path) = self.homebrew_lookup.get(file_name) {
                    return (Resolution::Required, path.clone());
                } else {
                    return (Resolution::Missing, PathBuf::from(file_path));
                }
            }

            if file_path == "/usr/local/lib/libobjc-env.dylib" {
                // Special case, ignore.
                return (Resolution::System, PathBuf::from(file_path));
            }
        } else if is_special {
            let file_name = file_path.rsplit('/').next().unwrap();

            if let Some(path) = self.sysroot_lookup.get(file_name) {
                return (Resolution::Sysroot, path.clone());
            }

            if let Some(path) = self.usr_lib_lookup.get(file_name) {
                return (Resolution::System, path.clone());
            }

            if let Some(path) = self.homebrew_lookup.get(file_name) {
                return (Resolution::Required, path.clone());
            }

            if file_path.ends_with("libc++.1.dylib") || file_path.ends_with("libz.1.dylib") {
                // Special case, ignore.
                return (Resolution::System, PathBuf::from(file_path));
            }
        }

        (Resolution::Unknown, PathBuf::from(file_path))
    }
}
