use crate::gc::{Gc, Trace};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use encoding_rs::Encoding;
use pyo3::exceptions::PyUnicodeError;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::PyDict;
use pyo3::{PyTraverseError, PyVisit};
use sugar_path::SugarPath;

use crate::template::django_rusty_templates::{Engine, Template, TemplateDoesNotExist};

static APPS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

fn normalize_newlines(contents: &str) -> Cow<'_, str> {
    if contents.contains('\r') {
        Cow::Owned(contents.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(contents)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoaderError {
    pub tried: Vec<(String, String)>,
}

fn abspath(path: &Path) -> Option<PathBuf> {
    match path.as_os_str().is_empty() {
        false => std::path::absolute(path)
            .map(|p| p.normalize().into_owned())
            .ok(),
        true => std::env::current_dir().ok(),
    }
}

fn safe_join(directory: &Path, template_name: &str) -> Option<PathBuf> {
    let final_path = abspath(&directory.join(template_name))?;
    let directory = abspath(directory)?;
    if final_path.starts_with(directory) {
        Some(final_path)
    } else {
        None
    }
}

fn get_app_template_dir(path: Bound<'_, PyAny>, dirname: &str) -> PyResult<Option<PathBuf>> {
    if path.is_truthy()? {
        let path_buf: PathBuf = path.extract()?;
        let template_path = path_buf.join(dirname);
        if template_path.is_dir() {
            return Ok(Some(template_path));
        }
    }
    Ok(None)
}

pub fn get_app_template_dirs(py: Python<'_>, dirname: &str) -> Result<Vec<PathBuf>, PyErr> {
    let apps = APPS.import(py, "django.apps", "apps")?;
    let app_configs = apps.call_method0("get_app_configs")?;

    let mut template_dirs = Vec::new();
    for app_config_result in app_configs.try_iter()? {
        let path = app_config_result?.getattr("path")?;
        if let Some(template_path) = get_app_template_dir(path, dirname)? {
            template_dirs.push(template_path);
        }
    }

    Ok(template_dirs)
}
#[derive(Debug)]
pub struct FileSystemLoader {
    pub dirs: Vec<PathBuf>,
    pub uses_engine_dirs: bool,
    encoding: &'static Encoding,
}

impl FileSystemLoader {
    pub fn new(dirs: Vec<PathBuf>, encoding: &'static Encoding) -> Self {
        Self {
            dirs,
            encoding,
            uses_engine_dirs: false,
        }
    }

    pub fn with_engine_dirs(dirs: Vec<PathBuf>, encoding: &'static Encoding) -> Self {
        Self {
            dirs,
            encoding,
            uses_engine_dirs: true,
        }
    }

    pub fn from_pathbuf(dirs: Vec<PathBuf>, encoding: &'static Encoding) -> Self {
        Self {
            dirs,
            encoding,
            uses_engine_dirs: false,
        }
    }

    fn find_template(
        &self,
        py: Python<'_>,
        template_name: &str,
        engine: &Engine,
    ) -> Result<PyResult<Lookup>, LoaderError> {
        let mut tried = Vec::new();
        let dirs: Cow<'_, [PathBuf]> = if self.uses_engine_dirs {
            match engine.dirs(py) {
                Ok(dirs) => Cow::Owned(dirs),
                Err(error) => return Ok(Err(error)),
            }
        } else {
            Cow::Borrowed(&self.dirs)
        };
        for template_dir in dirs.iter() {
            let Some(path) = safe_join(template_dir, template_name) else {
                continue;
            };
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    tried.push((
                        path.display().to_string(),
                        "Source does not exist".to_string(),
                    ));
                    continue;
                }
                Err(error) => return Ok(Err(error.into())),
            };
            if Encoding::for_label(engine.file_charset.as_bytes()).is_none() {
                let contents = pyo3::types::PyBytes::new(py, &bytes)
                    .call_method1("decode", (&engine.file_charset,))
                    .and_then(|value| value.extract::<String>());
                return Ok(contents.map(|contents| {
                    Lookup::source(normalize_newlines(&contents).into_owned(), path)
                }));
            }
            let (contents, encoding, malformed) = self.encoding.decode(&bytes);
            if malformed {
                return Ok(Err(PyUnicodeError::new_err(format!(
                    "Could not open {} with {} encoding.",
                    path.display(),
                    encoding.name()
                ))));
            }
            return Ok(Ok(Lookup::source(
                normalize_newlines(&contents).into_owned(),
                path,
            )));
        }
        Err(LoaderError { tried })
    }
}
#[derive(Debug)]
pub struct AppDirsLoader {
    encoding: &'static Encoding,
}

impl AppDirsLoader {
    pub fn new(encoding: &'static Encoding) -> Self {
        Self { encoding }
    }

    fn find_template(
        &self,
        py: Python<'_>,
        template_name: &str,
        engine: &Engine,
    ) -> Result<PyResult<Lookup>, LoaderError> {
        let dirs = match get_app_template_dirs(py, "templates") {
            Ok(dirs) => dirs,
            Err(e) => return Ok(Err(e)),
        };
        let filesystem_loader = FileSystemLoader::from_pathbuf(dirs, self.encoding);
        filesystem_loader.find_template(py, template_name, engine)
    }
}
/// The Python `get_template_cache` dict, kept in step with the native cache.
#[derive(Debug)]
pub struct CacheView {
    dict: Py<PyDict>,
    /// The cached loader's position in the engine's loader tree, prefixed to
    /// each entry's relative `loader_path` so the exposed origin is correct.
    path: Vec<usize>,
}

impl Trace for CacheView {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.dict.trace(visit)
    }
}

impl CacheView {
    /// Set `key`, returning the value it replaces.
    fn set_item(
        &self,
        py: Python<'_>,
        key: &str,
        entry: &Result<Template, LoaderError>,
        debug: bool,
    ) -> PyResult<Option<Py<PyAny>>> {
        let dict = self.dict.bind(py);
        let previous = dict.get_item(key)?.map(Bound::unbind);
        match entry {
            Ok(template) => {
                let mut template = template.clone();
                let mut path = self.path.clone();
                path.extend(&template.loader_path);
                template.loader_path = path;
                dict.set_item(key, template)
            }
            // Like Django's cached loader, debug mode stores an exception
            // instance and production stores the class.
            Err(_) if debug => dict.set_item(
                key,
                TemplateDoesNotExist::new_err(key.to_string()).value(py),
            ),
            Err(_) => dict.set_item(key, py.get_type::<TemplateDoesNotExist>()),
        }?;
        Ok(previous)
    }
}

/// Cache state removed while the loader lock is held. Python finalizers may
/// call back into a loader, so drop it only after releasing the lock.
#[must_use]
#[derive(Default)]
pub struct Discarded {
    _entries: Vec<Result<Template, LoaderError>>,
    _values: Vec<Py<PyAny>>,
}

#[derive(Debug)]
pub struct CachedLoader {
    pub cache: HashMap<String, Result<Template, LoaderError>>,
    pub cache_view: Option<CacheView>,
    pub loaders: Vec<Loader>,
}

impl CachedLoader {
    pub fn new(loaders: Vec<Loader>) -> Self {
        Self {
            loaders,
            cache: HashMap::new(),
            cache_view: None,
        }
    }

    /// Return the Python view of the cache, building it once from the native
    /// cache. Afterwards `store` and `clear` update it entry by entry, so
    /// reads are O(1) and never observe a partially rebuilt dict.
    pub fn view<'py>(
        &mut self,
        py: Python<'py>,
        path: &[usize],
        debug: bool,
    ) -> PyResult<Bound<'py, PyDict>> {
        if let Some(view) = &self.cache_view {
            return Ok(view.dict.bind(py).clone());
        }
        let view = CacheView {
            dict: PyDict::new(py).unbind(),
            path: path.to_vec(),
        };
        for (key, entry) in &self.cache {
            view.set_item(py, key, entry, debug)?;
        }
        let dict = view.dict.bind(py).clone();
        self.cache_view = Some(view);
        Ok(dict)
    }

    /// Insert a cache entry, mirroring it into the Python view if one exists.
    pub fn store(
        &mut self,
        py: Python<'_>,
        key: String,
        entry: Result<Template, LoaderError>,
        debug: bool,
    ) -> PyResult<Discarded> {
        let mut discarded = Discarded::default();
        if let Some(view) = &self.cache_view {
            discarded
                ._values
                .extend(view.set_item(py, &key, &entry, debug)?);
        }
        discarded._entries.extend(self.cache.insert(key, entry));
        Ok(discarded)
    }

    pub fn clear(&mut self, py: Python<'_>) -> Discarded {
        let mut discarded = Discarded {
            _entries: self.cache.drain().map(|(_, entry)| entry).collect(),
            _values: Vec::new(),
        };
        if let Some(view) = &self.cache_view {
            // Keep the dict itself: callers may hold it, as with Django's.
            let dict = view.dict.bind(py);
            discarded._values.push(dict.values().into_any().unbind());
            dict.clear();
        }
        discarded
    }

    /// A miss is cached immediately, but a found source is only cached once
    /// the caller has compiled it and passes it to [`Loader::cache_template`].
    fn find_template(
        &mut self,
        py: Python<'_>,
        template_name: &str,
        engine: &Engine,
    ) -> Result<PyResult<Lookup>, LoaderError> {
        match self.cache.get(template_name) {
            Some(Ok(template)) => Ok(Ok(Lookup::Cached(template.clone()))),
            Some(Err(e)) => Err(e.clone()),
            None => {
                let mut tried = Vec::new();
                for (index, loader) in self.loaders.iter_mut().enumerate() {
                    match loader.find_template(py, template_name, engine) {
                        Ok(Ok(mut lookup)) => {
                            lookup.loader_path_mut().insert(0, index);
                            if let Lookup::Cached(template) = &lookup {
                                // The name was not cached, so nothing substantial
                                // is discarded while the engine holds the lock.
                                if let Err(e) = self.store(
                                    py,
                                    template_name.to_string(),
                                    Ok(template.clone()),
                                    engine.debug(),
                                ) {
                                    return Ok(Err(e));
                                }
                            }
                            return Ok(Ok(lookup));
                        }
                        Ok(Err(e)) => return Ok(Err(e)),
                        Err(mut e) => tried.append(&mut e.tried),
                    }
                }
                let error = LoaderError { tried };
                if let Err(e) = self.store(
                    py,
                    template_name.to_string(),
                    Err(error.clone()),
                    engine.debug(),
                ) {
                    return Ok(Err(e));
                }
                Err(error)
            }
        }
    }

    /// Cache a template compiled from a [`Lookup::Source`] here and in every
    /// nested cached loader along `path`. Entries cached by another caller
    /// while this one compiled are kept, so nothing is discarded.
    fn cache_template(
        &mut self,
        py: Python<'_>,
        template_name: &str,
        template: &Template,
        path: &[usize],
        debug: bool,
    ) -> PyResult<()> {
        if !self.cache.contains_key(template_name) {
            let mut template = template.clone();
            template.loader_path = path.to_vec();
            let discarded = self.store(py, template_name.to_string(), Ok(template), debug)?;
            drop(discarded);
        }
        if let Some((index, rest)) = path.split_first()
            && let Loader::Cached(loader) = &mut self.loaders[*index]
        {
            loader.cache_template(py, template_name, template, rest, debug)?;
        }
        Ok(())
    }
}
#[derive(Debug)]
pub struct LocMemLoader {
    pub templates: HashMap<String, String>,
}

impl LocMemLoader {
    #[allow(dead_code)]
    pub fn new(templates: HashMap<String, String>) -> Self {
        Self { templates }
    }

    fn find_template(&self, template_name: &str) -> Result<PyResult<Lookup>, LoaderError> {
        if let Some(contents) = self.templates.get(template_name) {
            Ok(Ok(Lookup::source(
                contents.clone(),
                PathBuf::from(template_name),
            )))
        } else {
            Err(LoaderError {
                tried: vec![(
                    template_name.to_string(),
                    "Source does not exist".to_string(),
                )],
            })
        }
    }
}
#[derive(Debug)]
pub struct ExternalLoader {}

impl ExternalLoader {
    fn find_template(&self) -> Result<PyResult<Lookup>, LoaderError> {
        std::todo!() // Bail here because it does not make much sense to convert from PyErr to empty LoaderError
    }
}

#[derive(Debug)]
pub enum Loader {
    FileSystem(FileSystemLoader),
    AppDirs(AppDirsLoader),
    Cached(CachedLoader),
    #[allow(dead_code)]
    LocMem(LocMemLoader),
    #[allow(dead_code)]
    External(ExternalLoader),
}

impl Loader {
    /// Find `template_name` without compiling it.
    ///
    /// Compiling can run Python tag compile functions, which may load other
    /// templates from the same engine, so callers must not hold the engine's
    /// loader lock while they compile a [`Lookup::Source`].
    pub fn find_template(
        &mut self,
        py: Python<'_>,
        template_name: &str,
        engine: &Engine,
    ) -> Result<PyResult<Lookup>, LoaderError> {
        match self {
            Self::FileSystem(loader) => loader.find_template(py, template_name, engine),
            Self::AppDirs(loader) => loader.find_template(py, template_name, engine),
            Self::Cached(loader) => loader.find_template(py, template_name, engine),
            Self::LocMem(loader) => loader.find_template(template_name),
            Self::External(loader) => loader.find_template(),
        }
    }

    /// Store a template compiled from a [`Lookup::Source`] in every cached
    /// loader along its `loader_path`, which is relative to `self`.
    ///
    /// Entries cached by another caller while this one compiled are kept.
    pub fn cache_template(
        &mut self,
        py: Python<'_>,
        template_name: &str,
        template: &Template,
        debug: bool,
    ) -> PyResult<()> {
        if let Self::Cached(loader) = self {
            loader.cache_template(py, template_name, template, &template.loader_path, debug)?;
        }
        Ok(())
    }
}

/// The result of finding a template in a loader.
#[derive(Debug)]
pub enum Lookup {
    /// A template a cached loader has already compiled.
    Cached(Template),
    /// Source that still needs compiling.
    Source {
        contents: String,
        path: PathBuf,
        loader_path: Vec<usize>,
    },
}

impl Lookup {
    fn source(contents: String, path: PathBuf) -> Self {
        Self::Source {
            contents,
            path,
            loader_path: Vec::new(),
        }
    }

    fn loader_path_mut(&mut self) -> &mut Vec<usize> {
        match self {
            Self::Cached(template) => &mut template.loader_path,
            Self::Source { loader_path, .. } => loader_path,
        }
    }

    pub fn into_template(
        self,
        py: Python<'_>,
        template_name: &str,
        engine: Gc<Engine>,
    ) -> PyResult<Template> {
        match self {
            Self::Cached(template) => Ok(template),
            Self::Source {
                contents,
                path,
                loader_path,
            } => {
                let mut template = Template::new(py, &contents, path, template_name, engine)?;
                template.loader_path = loader_path;
                Ok(template)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::{BoundObject, IntoPyObjectExt};

    /// Find and compile a template the way the engine does, so the loaders
    /// can be tested on their own.
    trait GetTemplate {
        fn get_template(
            self,
            py: Python<'_>,
            template_name: &str,
            engine: Gc<Engine>,
        ) -> Result<PyResult<Template>, LoaderError>;
    }

    fn compile(
        py: Python<'_>,
        found: Result<PyResult<Lookup>, LoaderError>,
        template_name: &str,
        engine: Gc<Engine>,
    ) -> Result<PyResult<Template>, LoaderError> {
        Ok(found?.and_then(|lookup| lookup.into_template(py, template_name, engine)))
    }

    impl GetTemplate for &FileSystemLoader {
        fn get_template(
            self,
            py: Python<'_>,
            template_name: &str,
            engine: Gc<Engine>,
        ) -> Result<PyResult<Template>, LoaderError> {
            let found = self.find_template(py, template_name, &engine);
            compile(py, found, template_name, engine)
        }
    }

    impl GetTemplate for &AppDirsLoader {
        fn get_template(
            self,
            py: Python<'_>,
            template_name: &str,
            engine: Gc<Engine>,
        ) -> Result<PyResult<Template>, LoaderError> {
            let found = self.find_template(py, template_name, &engine);
            compile(py, found, template_name, engine)
        }
    }

    impl GetTemplate for &LocMemLoader {
        fn get_template(
            self,
            py: Python<'_>,
            template_name: &str,
            engine: Gc<Engine>,
        ) -> Result<PyResult<Template>, LoaderError> {
            compile(py, self.find_template(template_name), template_name, engine)
        }
    }

    impl GetTemplate for &mut CachedLoader {
        fn get_template(
            self,
            py: Python<'_>,
            template_name: &str,
            engine: Gc<Engine>,
        ) -> Result<PyResult<Template>, LoaderError> {
            let debug = engine.debug();
            Ok(self
                .find_template(py, template_name, &engine)?
                .and_then(|lookup| {
                    let template = lookup.into_template(py, template_name, engine)?;
                    let path = template.loader_path.clone();
                    self.cache_template(py, template_name, &template, &path, debug)?;
                    Ok(template)
                }))
        }
    }

    use proptest::prelude::*;

    fn setup_django(py: Python<'_>) {
        // Import the os module and set the DJANGO_SETTINGS_MODULE environment variable
        let os_module = PyModule::import(py, "os").unwrap();
        let environ = os_module.getattr("environ").unwrap();
        environ
            .call_method(
                "setdefault",
                ("DJANGO_SETTINGS_MODULE", "tests.settings"),
                None,
            )
            .unwrap();

        // Import the django module and call django.setup()
        let django_module = PyModule::import(py, "django").unwrap();
        django_module.call_method0("setup").unwrap();
    }

    #[test]
    fn test_filesystem_loader() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let loader =
                FileSystemLoader::new(vec![PathBuf::from("tests/templates")], encoding_rs::UTF_8);
            let template = loader
                .get_template(py, "basic.txt", engine)
                .unwrap()
                .unwrap();

            let mut expected = std::env::current_dir().unwrap();
            #[cfg(not(windows))]
            expected.push("tests/templates/basic.txt");
            #[cfg(windows)]
            expected.push("tests\\templates\\basic.txt");
            assert_eq!(template.filename.unwrap(), expected);
        });
    }

    #[test]
    fn test_filesystem_loader_missing_template() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let loader =
                FileSystemLoader::new(vec![PathBuf::from("tests/templates")], encoding_rs::UTF_8);
            let error = loader.get_template(py, "missing.txt", engine).unwrap_err();

            let mut expected = std::env::current_dir().unwrap();
            #[cfg(not(windows))]
            expected.push("tests/templates/missing.txt");
            #[cfg(windows)]
            expected.push("tests\\templates\\missing.txt");
            assert_eq!(
                error,
                LoaderError {
                    tried: vec![(
                        expected.display().to_string(),
                        "Source does not exist".to_string(),
                    )],
                },
            );
        });
    }

    #[test]
    fn test_filesystem_loader_invalid_encoding() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let loader =
                FileSystemLoader::new(vec![PathBuf::from("tests/templates")], encoding_rs::UTF_8);
            let error = loader
                .get_template(py, "invalid.txt", engine)
                .unwrap()
                .unwrap_err();

            let mut expected = std::env::current_dir().unwrap();
            #[cfg(not(windows))]
            expected.push("tests/templates/invalid.txt");
            #[cfg(windows)]
            expected.push("tests\\templates\\invalid.txt");
            assert_eq!(
                error.to_string(),
                format!(
                    "UnicodeError: Could not open {} with UTF-8 encoding.",
                    expected.display()
                )
            );
        });
    }

    #[test]
    fn test_cached_loader() {
        Python::initialize();

        Python::attach(|py| {
            // Helper to check cache contents
            let verify_cache = |cache: &HashMap<String, Result<Template, LoaderError>>,
                                key: &str,
                                expected_path: &Path| {
                if let Some(Ok(cached_template)) = cache.get(key) {
                    assert_eq!(cached_template.filename.as_ref().unwrap(), expected_path);
                } else {
                    panic!("Expected '{key}' to be in cache.");
                }
            };

            let engine = Gc::new(Engine::empty());

            // Create a FileSystemLoader for the CachedLoader
            let filesystem_loader =
                FileSystemLoader::new(vec![PathBuf::from("tests/templates")], encoding_rs::UTF_8);

            // Wrap the FileSystemLoader in a CachedLoader
            let mut cached_loader = CachedLoader::new(vec![Loader::FileSystem(filesystem_loader)]);

            // Load a template via the CachedLoader
            let template = cached_loader
                .get_template(py, "basic.txt", engine.clone())
                .expect("Failed to load template")
                .expect("Template file could not be read");

            // Verify the template filename
            let mut expected_path =
                std::env::current_dir().expect("Failed to get current directory");
            #[cfg(not(windows))]
            expected_path.push("tests/templates/basic.txt");
            #[cfg(windows)]
            expected_path.push("tests\\templates\\basic.txt");
            assert_eq!(template.filename.unwrap(), expected_path);

            // Verify the cache state after first load
            assert_eq!(cached_loader.cache.len(), 1);
            verify_cache(&cached_loader.cache, "basic.txt", &expected_path);

            // Load the same template again via the CachedLoader
            let template = cached_loader
                .get_template(py, "basic.txt", engine)
                .expect("Failed to load template")
                .expect("Template file could not be read");

            // Verify the template filename again
            assert_eq!(template.filename.unwrap(), expected_path);

            // Verify the cache state remains consistent
            assert_eq!(cached_loader.cache.len(), 1);
            verify_cache(&cached_loader.cache, "basic.txt", &expected_path);
        });
    }

    #[test]
    fn test_cached_loader_missing_template() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let filesystem_loader =
                FileSystemLoader::new(vec![PathBuf::from("tests/templates")], encoding_rs::UTF_8);

            let mut cached_loader = CachedLoader::new(vec![Loader::FileSystem(filesystem_loader)]);
            let error = cached_loader
                .get_template(py, "missing.txt", engine.clone())
                .unwrap_err();

            let mut expected = std::env::current_dir().unwrap();
            #[cfg(not(windows))]
            expected.push("tests/templates/missing.txt");
            #[cfg(windows)]
            expected.push("tests\\templates\\missing.txt");
            let expected_err = LoaderError {
                tried: vec![(
                    expected.display().to_string(),
                    "Source does not exist".to_string(),
                )],
            };
            assert_eq!(error, expected_err);

            let cache = &cached_loader.cache;
            assert_eq!(
                cache.get("missing.txt").unwrap().as_ref().unwrap_err(),
                &expected_err
            );

            let error = cached_loader
                .get_template(py, "missing.txt", engine)
                .unwrap_err();
            assert_eq!(error, expected_err);
        });
    }

    #[test]
    fn test_cached_loader_invalid_encoding() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let filesystem_loader =
                FileSystemLoader::new(vec![PathBuf::from("tests/templates")], encoding_rs::UTF_8);

            let mut cached_loader = CachedLoader::new(vec![Loader::FileSystem(filesystem_loader)]);
            let error = cached_loader
                .get_template(py, "invalid.txt", engine)
                .unwrap()
                .unwrap_err();

            let mut expected = std::env::current_dir().unwrap();
            #[cfg(not(windows))]
            expected.push("tests/templates/invalid.txt");
            #[cfg(windows)]
            expected.push("tests\\templates\\invalid.txt");
            assert_eq!(
                error.to_string(),
                format!(
                    "UnicodeError: Could not open {} with UTF-8 encoding.",
                    expected.display()
                )
            );
        });
    }

    #[test]
    fn test_locmem_loader() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let mut templates: HashMap<String, String> = HashMap::new();
            templates.insert("index.html".to_string(), "index".to_string());

            let loader = LocMemLoader::new(templates);

            let template = loader
                .get_template(py, "index.html", engine)
                .unwrap()
                .unwrap();
            assert_eq!(template.template.as_ref(), "index");
            assert_eq!(template.filename.unwrap(), PathBuf::from("index.html"));
        });
    }

    #[test]
    fn test_locmem_loader_missing_template() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let templates: HashMap<String, String> = HashMap::new();

            let loader = LocMemLoader::new(templates);

            let error = loader.get_template(py, "index.html", engine).unwrap_err();
            assert_eq!(
                error,
                LoaderError {
                    tried: vec![(
                        "index.html".to_string(),
                        "Source does not exist".to_string(),
                    )],
                },
            );
        });
    }

    #[test]
    fn test_appdirs_loader() {
        Python::initialize();

        Python::attach(|py| {
            // Setup Django
            setup_django(py);

            let engine = Gc::new(Engine::empty());
            let loader = AppDirsLoader::new(encoding_rs::UTF_8);
            let template = loader
                .get_template(py, "basic.txt", engine)
                .unwrap()
                .unwrap();

            let mut expected = std::env::current_dir().unwrap();
            #[cfg(not(windows))]
            expected.push("tests/templates/basic.txt");
            #[cfg(windows)]
            expected.push("tests\\templates\\basic.txt");
            assert_eq!(template.filename.unwrap(), expected);
        });
    }

    #[test]
    fn test_appdirs_loader_missing_template() {
        Python::initialize();

        Python::attach(|py| {
            // Setup Django
            setup_django(py);

            let engine = Gc::new(Engine::empty());
            let loader = AppDirsLoader::new(encoding_rs::UTF_8);
            let error = loader.get_template(py, "missing.txt", engine).unwrap_err();

            let mut expected = std::env::current_dir().unwrap();
            #[cfg(not(windows))]
            expected.push("tests/templates/missing.txt");
            #[cfg(windows)]
            expected.push("tests\\templates\\missing.txt");

            let auth = py.import("django.contrib.auth").unwrap();
            let auth: PathBuf = auth.getattr("__file__").unwrap().extract().unwrap();
            let mut auth = auth.parent().unwrap().to_path_buf();
            #[cfg(not(windows))]
            auth.push("templates/missing.txt");
            #[cfg(windows)]
            auth.push("templates\\missing.txt");

            assert_eq!(
                error,
                LoaderError {
                    tried: vec![
                        (
                            expected.display().to_string(),
                            "Source does not exist".to_string(),
                        ),
                        (
                            auth.display().to_string(),
                            "Source does not exist".to_string(),
                        ),
                    ],
                },
            );
        });
    }

    #[test]
    fn test_appdirs_loader_invalid_encoding() {
        Python::initialize();

        Python::attach(|py| {
            // Setup Django
            setup_django(py);

            let engine = Gc::new(Engine::empty());
            let loader = AppDirsLoader::new(encoding_rs::UTF_8);
            let error = loader
                .get_template(py, "invalid.txt", engine)
                .unwrap()
                .unwrap_err();

            let mut expected = std::env::current_dir().unwrap();
            #[cfg(not(windows))]
            expected.push("tests/templates/invalid.txt");
            #[cfg(windows)]
            expected.push("tests\\templates\\invalid.txt");
            assert_eq!(
                error.to_string(),
                format!(
                    "UnicodeError: Could not open {} with UTF-8 encoding.",
                    expected.display()
                )
            );
        });
    }

    #[test]
    fn test_get_app_template_dir_special_cases() {
        Python::initialize();

        Python::attach(|py| {
            // Test with None path
            let none_path = py.None().into_bound(py);
            let result = get_app_template_dir(none_path, "templates");
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), None);

            // Test with invalid type (integer)
            let invalid = 42.into_bound_py_any(py).unwrap();
            let result = get_app_template_dir(invalid, "templates");
            assert!(result.is_err());
        });
    }

    #[test]
    fn test_get_app_template_dir_with_str() {
        Python::initialize();

        Python::attach(|py| {
            // Test with Python string for current directory (nonexistent template)
            let current_dir = ".".into_bound_py_any(py).unwrap();
            let result = get_app_template_dir(current_dir, "nonexistent");
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), None);

            // Test with Python string for the "tests" directory
            let tests_dir = "tests".into_bound_py_any(py).unwrap();
            let result = get_app_template_dir(tests_dir, "templates");
            assert!(result.is_ok());
            let expected_path = PathBuf::from("tests").join("templates");
            assert_eq!(result.unwrap(), Some(expected_path));
        });
    }

    #[test]
    fn test_get_app_template_dir_with_pathlib() {
        Python::initialize();

        Python::attach(|py| {
            // Import pathlib.Path
            let path_module = py.import("pathlib").unwrap();
            let path_cls = path_module.getattr("Path").unwrap();

            // Test with pathlib.Path for current directory (nonexistent template)
            let path_obj = path_cls.call1((".",)).unwrap().into_bound();
            let result = get_app_template_dir(path_obj, "nonexistent");
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), None);

            // Test with pathlib.Path for the "tests" directory
            let path_obj = path_cls.call1(("tests",)).unwrap().into_bound();
            let result = get_app_template_dir(path_obj, "templates");
            assert!(result.is_ok());
            let expected_path = PathBuf::from("tests").join("templates");
            assert_eq!(result.unwrap(), Some(expected_path));
        });
    }

    #[test]
    fn test_get_app_template_dirs() {
        Python::initialize();

        Python::attach(|py| {
            // Setup Django
            setup_django(py);

            let dirs = get_app_template_dirs(py, "templates").unwrap();
            assert_eq!(dirs.len(), 2);

            let mut expected = std::env::current_dir().unwrap();
            expected.push("tests/templates");
            assert_eq!(dirs[0], expected);
        });
    }

    #[test]
    fn test_safe_join_absolute() {
        let path = PathBuf::from("/abc/");
        let joined = safe_join(&path, "def").unwrap();
        #[cfg(not(windows))]
        assert_eq!(joined, PathBuf::from("/abc/def"));
        #[cfg(windows)]
        assert!(joined.ends_with("\\abc\\def"));
    }

    #[test]
    fn test_safe_join_relative() {
        let path = PathBuf::from("abc");
        let joined = safe_join(&path, "def").unwrap();
        let mut expected = std::env::current_dir().unwrap();
        expected.push("abc/def");
        assert_eq!(joined, expected);
    }

    #[test]
    fn test_safe_join_absolute_starts_with_sep() {
        let path = PathBuf::from("/abc/");
        let joined = safe_join(&path, "/def");
        assert_eq!(joined, None);
    }

    #[test]
    fn test_safe_join_relative_starts_with_sep() {
        let path = PathBuf::from("abc");
        let joined = safe_join(&path, "/def");
        assert_eq!(joined, None);
    }

    #[test]
    fn test_safe_join_absolute_parent() {
        let path = PathBuf::from("/abc/");
        let joined = safe_join(&path, "../def");
        assert_eq!(joined, None);
    }

    #[test]
    fn test_safe_join_relative_parent() {
        let path = PathBuf::from("abc");
        let joined = safe_join(&path, "../def");
        assert_eq!(joined, None);
    }

    #[test]
    fn test_safe_join_absolute_parent_starts_with_sep() {
        let path = PathBuf::from("/abc/");
        let joined = safe_join(&path, "/../def");
        assert_eq!(joined, None);
    }

    #[test]
    fn test_safe_join_relative_parent_starts_with_sep() {
        let path = PathBuf::from("abc");
        let joined = safe_join(&path, "/../def");
        assert_eq!(joined, None);
    }

    #[test]
    fn test_safe_join_django_example() {
        let path = PathBuf::from("/dir");
        let joined = safe_join(&path, "/../d");
        assert_eq!(joined, None);
    }

    #[test]
    fn test_safe_join_django_example_variant() {
        let path = PathBuf::from("/dir");
        let joined = safe_join(&path, "/../directory");
        assert_eq!(joined, None);
    }

    #[test]
    fn test_safe_join_empty_path() {
        let path = PathBuf::from("");
        let joined = safe_join(&path, "directory").unwrap();
        let mut expected = std::env::current_dir().unwrap();
        expected.push("directory");
        assert_eq!(joined, expected);
    }

    #[test]
    fn test_safe_join_empty_path_and_template_name() {
        let path = PathBuf::from("");
        let joined = safe_join(&path, "").unwrap();
        let expected = std::env::current_dir().unwrap();
        assert_eq!(joined, expected);
    }

    #[test]
    fn test_safe_join_parent_and_empty_template_name() {
        let path = PathBuf::from("..");
        let joined = safe_join(&path, "").unwrap();
        let mut expected = std::env::current_dir().unwrap();
        expected.push("..");
        assert_eq!(joined, expected.normalize());
    }

    #[test]
    #[cfg_attr(
        windows,
        ignore = "Skipping on Windows due to path character restrictions"
    )]
    fn test_safe_join_matches_django_safe_join() {
        Python::initialize();
        proptest!(|(path: PathBuf, template_name: String)| {
            Python::attach(|py| {
                let utils_os = PyModule::import(py, "django.utils._os").unwrap();
                let django_safe_join = utils_os.getattr("safe_join").unwrap();

                let joined = django_safe_join
                    .call1((&path, &template_name))
                    .map(|joined| joined.extract::<PathBuf>().unwrap_or_default())
                    .ok();
                prop_assert_eq!(joined, safe_join(&path, &template_name));
                Ok(())
            })?;
        });
    }
}
