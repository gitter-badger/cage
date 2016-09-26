//! A conductor project.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::slice;

use default_tags::DefaultTags;
use dir;
use ext::file::FileExt;
use ovr::Override;
use plugins::{self, Operation};
use pod::{Pod, PodType};
use repos::Repos;
use rustc_serialize::json::{Json, ToJson};
use util::{ConductorPathExt, Error, ToStrOrErr};

/// A `conductor` project, which is represented as a directory containing a
/// `pods` subdirectory.
#[derive(Debug)]
pub struct Project {
    /// The name of this project.  This defaults to the name of the
    /// directory containing the project, but it can be overriden, just
    /// like with `docker-compose`.
    name: String,

    /// The directory which contains our `project`.  Must have a
    /// subdirectory named `pods`.
    root_dir: PathBuf,

    /// Where we keep cloned git repositories.
    src_dir: PathBuf,

    /// The directory to which we'll write our transformed pods.  Defaults
    /// to `root_dir.join(".conductor")`.
    output_dir: PathBuf,

    /// All the pods associated with this project.
    pods: Vec<Pod>,

    /// All the overrides associated with this project.
    overrides: Vec<Override>,

    /// All the repositories associated with this project.
    repos: Repos,

    /// Docker image tags to use for images that don't have them.
    /// Typically used to lock down versions supplied by a CI system.
    default_tags: Option<DefaultTags>,

    /// The plugins associated with this project.  Guaranteed to never be
    /// `None` after returning from `from_dirs`.
    plugins: Option<plugins::Manager>,
}

impl Project {
    /// Create a `Project`, specifying what directories to use.
    fn from_dirs(root_dir: &Path,
                 src_dir: &Path,
                 output_dir: &Path)
                 -> Result<Project, Error> {
        let overrides = try!(Project::find_overrides(root_dir));
        let pods = try!(Project::find_pods(root_dir, &overrides));
        let repos = try!(Repos::new(&pods));
        let absolute_root = try!(root_dir.to_absolute());
        let name = try!(absolute_root.file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| {
                    err!("Can't find directory name for {}", root_dir.display())
                }));
        let mut proj = Project {
            name: name.to_owned(),
            root_dir: root_dir.to_owned(),
            src_dir: src_dir.to_owned(),
            output_dir: output_dir.to_owned(),
            pods: pods,
            overrides: overrides,
            repos: repos,
            default_tags: None,
            plugins: None,
        };
        let plugins = try!(plugins::Manager::new(&proj));
        proj.plugins = Some(plugins);
        Ok(proj)
    }

    /// Create a `Project` using the current directory as input and the
    /// `.conductor` subdirectory as output.
    ///
    /// ```
    /// use conductor::Project;
    /// use std::env;
    ///
    /// let saved = env::current_dir().unwrap();
    /// env::set_current_dir("examples/hello/pods").unwrap();
    ///
    /// let proj = Project::from_current_dir().unwrap();
    /// assert_eq!(proj.root_dir(), saved.join("examples/hello"));
    /// assert_eq!(proj.src_dir(), saved.join("examples/hello/src"));
    /// assert_eq!(proj.output_dir(), saved.join("examples/hello/.conductor"));
    ///
    /// env::set_current_dir(saved).unwrap();
    /// ```
    pub fn from_current_dir() -> Result<Project, Error> {
        // (We can only test this using a doc test because testing it
        // requires messing with `set_current_dir`, which isn't thread safe
        // and will break parallel tests.)
        let current = try!(env::current_dir());
        let root_dir = try!(dir::find_project(&current));
        Project::from_dirs(&root_dir,
                           &root_dir.join("src"),
                           &root_dir.join(".conductor"))
    }

    /// (Tests only.) Create a `Project` from a subirectory of `examples`,
    /// with an output directory under `target/test_output/$NAME`.
    #[cfg(test)]
    pub fn from_example(name: &str) -> Result<Project, Error> {
        use rand::random;
        let root_dir = Path::new("examples").join(name);
        let rand_name = format!("{}-{}", name, random::<u16>());
        let test_output = Path::new("target/test_output").join(&rand_name);
        Project::from_dirs(&root_dir, &test_output.join("src"), &test_output)
    }

    /// (Tests only.) Remove our output directory after a test.
    #[cfg(test)]
    pub fn remove_test_output(&self) -> Result<(), Error> {
        if self.output_dir.exists() {
            try!(fs::remove_dir_all(&self.output_dir));
        }
        Ok(())
    }

    /// Find all the overrides defined in this project.
    fn find_overrides(root_dir: &Path) -> Result<Vec<Override>, Error> {
        let overrides_dir = root_dir.join("pods/overrides");
        let mut overrides = vec![];
        for glob_result in try!(overrides_dir.glob("*")) {
            let path = try!(glob_result);
            if path.is_dir() {
                // It's safe to unwrap file_name because we know it matched
                // our glob.
                let name = try!(path.file_name().unwrap().to_str_or_err()).to_owned();
                overrides.push(Override::new(name));
            }
        }
        Ok(overrides)
    }

    /// Find all the pods defined in this project.
    fn find_pods(root_dir: &Path, overrides: &[Override]) -> Result<Vec<Pod>, Error> {
        let pods_dir = root_dir.join("pods");
        let mut pods = vec![];
        for glob_result in try!(pods_dir.glob("*.yml")) {
            let path = try!(glob_result);
            // It's safe to unwrap the file_stem because we know it matched
            // our glob.
            let name = try!(path.file_stem().unwrap().to_str_or_err()).to_owned();
            pods.push(try!(Pod::new(pods_dir.clone(), name, overrides)));
        }
        Ok(pods)
    }

    /// The name of this project.  This defaults to the name of the current
    /// directory.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Set the name of this project.  This should be done before calling
    /// `output` or any methods in `conductor::cmd`.
    pub fn set_name(&mut self, name: &str) -> &mut Project {
        self.name = name.to_owned();
        self
    }

    /// The root directory of this project.
    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    /// The source directory of this project, where we can put cloned git
    /// repositories.
    pub fn src_dir(&self) -> &Path {
        &self.src_dir
    }

    /// The output directory of this project.  Normally `.conductor` inside
    /// the `root_dir`, but it may be overriden.
    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    /// The directory in which are pods are defined, and relative to which
    /// all `docker-compose.yml` paths should be interpreted.
    pub fn pods_dir(&self) -> PathBuf {
        self.root_dir.join("pods")
    }

    /// The path relative to which our pods will be output.  This can be
    /// joined with `Pod::rel_path` to get an output path for a specific pod.
    pub fn output_pods_dir(&self) -> PathBuf {
        self.output_dir.join("pods")
    }

    /// Iterate over all pods in this project.
    pub fn pods(&self) -> Pods {
        Pods { iter: self.pods.iter() }
    }

    /// Look up the named pod.
    pub fn pod(&self, name: &str) -> Option<&Pod> {
        // TODO LOW: Do we want to store pods in a BTreeMap by name?
        self.pods().find(|pod| pod.name() == name)
    }

    /// Iterate over all overlays in this project.
    pub fn overrides(&self) -> Overrides {
        Overrides { iter: self.overrides.iter() }
    }

    /// Look up the named override.  We name this function `ovr` instead of
    /// `override` to avoid a keyword clash.
    pub fn ovr(&self, name: &str) -> Option<&Override> {
        self.overrides().find(|ovr| ovr.name() == name)
    }

    /// Return the collection of git repositories associated with this
    /// project.
    pub fn repos(&self) -> &Repos {
        &self.repos
    }

    /// Get the default tags associated with this project, if any.
    pub fn default_tags(&self) -> Option<&DefaultTags> {
        self.default_tags.as_ref()
    }

    /// Set the default tags associated with this project.
    pub fn set_default_tags(&mut self, tags: DefaultTags) -> &mut Project {
        self.default_tags = Some(tags);
        self
    }

    /// Our plugin manager.
    pub fn plugins(&self) -> &plugins::Manager {
        self.plugins
            .as_ref()
            .expect("plugins should always be set at Project init")
    }

    /// Process our pods, flattening and transforming them using our
    /// plugins, and output them to the specified directory.
    fn output_helper(&self,
                     ovr: &Override,
                     op: Operation,
                     export_dir: &Path)
                     -> Result<(), Error> {
        // Output each pod.
        for pod in &self.pods {
            // Figure out where to put our pod.
            let file_name = format!("{}.yml", pod.name());
            let rel_path = match (op, try!(pod.pod_type(ovr))) {
                (Operation::Export, PodType::Task) => {
                    Path::new("tasks").join(file_name)
                }
                _ => Path::new(&file_name).to_owned(),
            };
            let out_path = try!(export_dir.join(&rel_path).with_guaranteed_parent());
            debug!("Outputting {}", out_path.display());

            // Combine overrides, make it standalone, tweak as needed, and
            // output.
            let mut file = try!(pod.merged_file(ovr));
            try!(file.make_standalone(&self.pods_dir()));
            match op {
                Operation::Output => try!(file.update_for_output(self)),
                Operation::Export => try!(file.update_for_export(self)),
            }
            let ctx = plugins::Context::new(self, ovr, pod);
            try!(self.plugins().transform(op, &ctx, &mut file));
            try!(file.write_to_path(out_path));
        }
        Ok(())
    }


    /// Delete our existing output and replace it with a processed and
    /// expanded version of our pod definitions.
    pub fn output(&self, ovr: &Override) -> Result<(), Error> {
        // Get a path to our output pods directory (and delete it if it
        // exists).
        let out_pods = self.output_pods_dir();
        if out_pods.exists() {
            try!(fs::remove_dir_all(&out_pods)
                .map_err(|e| err!("Cannot delete {}: {}", out_pods.display(), e)));
        }

        self.output_helper(ovr, Operation::Output, &out_pods)
    }

    /// Export this project (with the specified override applied) as a set
    /// of standalone `*.yml` files with no environment variable
    /// interpolations and no external dependencies.
    pub fn export(&self, ovr: &Override, export_dir: &Path) -> Result<(), Error> {
        // Don't clobber an existing directory.
        if export_dir.exists() {
            return Err(err!("The directory {} already exists", export_dir.display()));
        }

        // You should really supply default tags if you're going to export.
        if self.default_tags().is_none() {
            warn!("Exporting project without --default-tags");
        }

        self.output_helper(ovr, Operation::Export, export_dir)
    }
}

/// Convert to JSON for use in generator templates.
impl<'a> ToJson for Project {
    fn to_json(&self) -> Json {
        let mut info: BTreeMap<String, Json> = BTreeMap::new();
        info.insert("name".to_string(), self.name().to_json());
        info.to_json()
    }
}

/// An iterator over the pods in a project.
#[derive(Debug, Clone)]
pub struct Pods<'a> {
    /// Our wrapped iterator.  We wrap this in our own struct to make the
    /// underlying type opaque.
    iter: slice::Iter<'a, Pod>,
}

impl<'a> Iterator for Pods<'a> {
    type Item = &'a Pod;

    fn next(&mut self) -> Option<&'a Pod> {
        self.iter.next()
    }
}

/// An iterator over the overrides in a project.
#[derive(Debug, Clone)]
pub struct Overrides<'a> {
    /// Our wrapped iterator.  We wrap this in our own struct to make the
    /// underlying type opaque.
    iter: slice::Iter<'a, Override>,
}

impl<'a> Iterator for Overrides<'a> {
    type Item = &'a Override;

    fn next(&mut self) -> Option<&'a Override> {
        self.iter.next()
    }
}

#[test]
fn new_from_example_uses_example_and_target() {
    use env_logger;
    let _ = env_logger::init();
    let proj = Project::from_example("hello").unwrap();
    assert_eq!(proj.root_dir, Path::new("examples/hello"));
    let output_dir = proj.output_dir.to_str_or_err().unwrap();
    assert!(output_dir.starts_with("target/test_output/hello-"));
    let src_dir = proj.src_dir.to_str_or_err().unwrap();
    assert!(src_dir.starts_with("target/test_output/hello-"));
}

#[test]
fn name_defaults_to_project_dir_but_can_be_overridden() {
    use env_logger;
    let _ = env_logger::init();
    let mut proj = Project::from_example("hello").unwrap();
    assert_eq!(proj.name(), "hello");
    proj.set_name("hi");
    assert_eq!(proj.name(), "hi");
}

#[test]
fn pods_are_loaded() {
    use env_logger;
    let _ = env_logger::init();
    let proj = Project::from_example("hello").unwrap();
    let names: Vec<_> = proj.pods.iter().map(|pod| pod.name()).collect();
    assert_eq!(names, ["frontend"]);
}

#[test]
fn overrides_are_loaded() {
    use env_logger;
    let _ = env_logger::init();
    let proj = Project::from_example("hello").unwrap();
    let names: Vec<_> = proj.overrides.iter().map(|o| o.name()).collect();
    assert_eq!(names, ["development", "production", "test"]);
}

#[test]
fn output_creates_a_directory_of_flat_yml_files() {
    use env_logger;
    let _ = env_logger::init();
    let proj = Project::from_example("rails_hello").unwrap();
    let ovr = proj.ovr("development").unwrap();
    proj.output(ovr).unwrap();
    assert!(proj.output_dir.join("pods/frontend.yml").exists());
    assert!(proj.output_dir.join("pods/db.yml").exists());
    assert!(proj.output_dir.join("pods/migrate.yml").exists());
    proj.remove_test_output().unwrap();
}

#[test]
fn export_creates_a_directory_of_flat_yml_files() {
    use env_logger;
    let _ = env_logger::init();
    let proj = Project::from_example("rails_hello").unwrap();
    let export_dir = proj.output_dir.join("hello_export");
    let ovr = proj.ovr("development").unwrap();
    proj.export(ovr, &export_dir).unwrap();
    assert!(export_dir.join("frontend.yml").exists());
    assert!(export_dir.join("db.yml").exists());
    assert!(export_dir.join("tasks/migrate.yml").exists());
    proj.remove_test_output().unwrap();
}
