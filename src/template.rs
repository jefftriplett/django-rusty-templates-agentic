use pyo3::prelude::*;

#[pymodule]
pub mod django_rusty_templates {
    use std::borrow::Cow;
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};

    use encoding_rs::Encoding;
    use pyo3::exceptions::{
        PyAttributeError, PyException, PyImportError, PyNotImplementedError, PyOverflowError,
        PyTypeError, PyValueError,
    };
    use pyo3::import_exception;
    use pyo3::intern;
    use pyo3::prelude::*;
    use pyo3::sync::{MutexExt, PyOnceLock};
    use pyo3::types::{PyBool, PyDict, PyIterator, PyList, PyString, PyTuple};

    use crate::error::{PyRenderError, RenderError};
    use crate::loaders::{AppDirsLoader, CachedLoader, FileSystemLoader, Loader, LocMemLoader};
    use crate::parse::{Parser, PartialDefinition, Tag, TokenTree};
    use crate::render::types::{Context, PyContext};
    use crate::render::{Render, RenderResult};
    use crate::utils::PyResultMethods;
    use dtl_lexer::types::TemplateString;

    import_exception!(django.core.exceptions, ImproperlyConfigured);
    import_exception!(django.template.base, VariableDoesNotExist);
    import_exception!(django.template.exceptions, TemplateDoesNotExist);
    import_exception!(django.template.exceptions, TemplateSyntaxError);
    import_exception!(django.template.library, InvalidTemplateLibrary);
    import_exception!(django.urls, NoReverseMatch);

    static DJANGO_CONTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    static TEMPLATE_RENDERED: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    static IMPORT_STRING: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    static MARK_SAFE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    /// Experimental JSON-only runtime sharing its parser/renderer with browser Wasm.
    #[pyclass]
    pub struct PortableEngine {
        inner: dtl_core::Engine,
    }

    #[pymethods]
    impl PortableEngine {
        #[new]
        fn new() -> Self {
            Self {
                inner: dtl_core::Engine::new(),
            }
        }

        fn add_template(&mut self, name: &str, source: &str) -> PyResult<()> {
            self.inner
                .add_template(name, source)
                .map_err(PyValueError::new_err)
        }

        fn render_json(&self, name: &str, context_json: &str) -> PyResult<String> {
            self.inner
                .render(name, context_json)
                .map_err(PyValueError::new_err)
        }
    }

    trait WithSourceCode {
        fn with_source_code(
            err: miette::Report,
            source: impl miette::SourceCode + 'static,
        ) -> PyErr;
    }

    impl WithSourceCode for TemplateSyntaxError {
        fn with_source_code(
            err: miette::Report,
            source: impl miette::SourceCode + 'static,
        ) -> PyErr {
            let miette_err = err.with_source_code(source);
            Self::new_err(format!("{miette_err:?}"))
        }
    }

    impl WithSourceCode for VariableDoesNotExist {
        fn with_source_code(
            err: miette::Report,
            source: impl miette::SourceCode + 'static,
        ) -> PyErr {
            let miette_err = err.with_source_code(source);
            let report = format!("{miette_err:?}");
            // Work around old-style Python formatting in VariableDoesNotExist.__str__
            let report = report.replace('%', "%%");
            Self::new_err(report)
        }
    }

    impl WithSourceCode for PyOverflowError {
        fn with_source_code(
            err: miette::Report,
            source: impl miette::SourceCode + 'static,
        ) -> PyErr {
            let miette_err = err.with_source_code(source);
            Self::new_err(format!("{miette_err:?}"))
        }
    }

    impl WithSourceCode for PyValueError {
        fn with_source_code(
            err: miette::Report,
            source: impl miette::SourceCode + 'static,
        ) -> PyErr {
            let miette_err = err.with_source_code(source);
            Self::new_err(format!("{miette_err:?}"))
        }
    }

    #[derive(Clone)]
    #[pyclass(frozen, skip_from_py_object)]
    struct LoaderView {
        engine: Arc<Engine>,
        path: Vec<usize>,
    }

    impl PartialEq for LoaderView {
        fn eq(&self, other: &Self) -> bool {
            Arc::ptr_eq(&self.engine, &other.engine) && self.path == other.path
        }
    }

    fn loader_at<'a>(loaders: &'a mut [Loader], path: &[usize]) -> &'a mut Loader {
        let (index, rest) = path.split_first().expect("loader path is nonempty");
        let loader = &mut loaders[*index];
        if rest.is_empty() {
            return loader;
        }
        let Loader::Cached(loader) = loader else {
            unreachable!("only cached loaders have children")
        };
        loader_at(&mut loader.loaders, rest)
    }

    impl LoaderView {
        fn with_loader<R>(&self, py: Python<'_>, operation: impl FnOnce(&mut Loader) -> R) -> R {
            let mut loaders = self
                .engine
                .template_loaders
                .lock_py_attached(py)
                .expect("Mutex should not be poisoned");
            operation(loader_at(&mut loaders, &self.path))
        }

        fn name(&self, py: Python<'_>) -> &'static str {
            self.with_loader(py, |loader| match loader {
                Loader::FileSystem(_) => "django.template.loaders.filesystem.Loader",
                Loader::AppDirs(_) => "django.template.loaders.app_directories.Loader",
                Loader::Cached(_) => "django.template.loaders.cached.Loader",
                Loader::LocMem(_) => "django.template.loaders.locmem.Loader",
                Loader::External(_) => "external.Loader",
            })
        }
    }

    #[pymethods]
    impl LoaderView {
        fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
            let other = other.getattr("_native").unwrap_or_else(|_| other.clone());
            other
                .extract::<PyRef<'_, Self>>()
                .is_ok_and(|other| self == &*other)
        }

        #[getter]
        fn is_cached(&self, py: Python<'_>) -> bool {
            self.with_loader(py, |loader| matches!(loader, Loader::Cached(_)))
        }

        #[getter]
        fn loaders(&self, py: Python<'_>) -> Vec<Self> {
            let count = self.with_loader(py, |loader| match loader {
                Loader::Cached(loader) => loader.loaders.len(),
                _ => 0,
            });
            (0..count)
                .map(|index| {
                    let mut path = self.path.clone();
                    path.push(index);
                    Self {
                        engine: self.engine.clone(),
                        path,
                    }
                })
                .collect()
        }

        fn get_dirs<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
            let uses_engine_dirs = self.with_loader(
                py,
                |loader| matches!(loader, Loader::FileSystem(loader) if loader.uses_engine_dirs),
            );
            if uses_engine_dirs && let Some(configuration) = &self.engine.configuration {
                return Ok(configuration
                    .bind(py)
                    .get_item("dirs")?
                    .expect("configured dirs"));
            }
            let own = self.with_loader(py, |loader| match loader {
                Loader::FileSystem(loader) => Some(loader.dirs.clone()),
                _ => None,
            });
            if let Some(dirs) = own {
                if let Some(configuration) = &self.engine.configuration {
                    let mut configured = configuration
                        .bind(py)
                        .get_item("loaders")?
                        .expect("configured loaders");
                    for (position, index) in self.path.iter().enumerate() {
                        configured = configured.get_item(*index)?;
                        if position + 1 < self.path.len() {
                            configured = configured.get_item(1)?;
                        }
                    }
                    if !configured.is_instance_of::<PyString>() {
                        return configured.get_item(1);
                    }
                }
                return Ok(PyList::new(
                    py,
                    dirs.iter().map(|directory| directory.to_string_lossy()),
                )?
                .into_any());
            }
            if self.name(py) == "django.template.loaders.app_directories.Loader" {
                return Ok(PyList::new(
                    py,
                    crate::loaders::get_app_template_dirs(py, "templates")?,
                )?
                .into_any());
            }
            let dirs = PyList::empty(py);
            for loader in self.loaders(py) {
                for directory in loader.get_dirs(py)?.try_iter()? {
                    dirs.append(directory?)?;
                }
            }
            Ok(dirs.into_any())
        }

        fn reset(&self, py: Python<'_>) {
            self.with_loader(py, |loader| {
                if let Loader::Cached(loader) = loader {
                    loader.cache.clear();
                    if let Some(view) = loader.cache_view.as_ref() {
                        view.bind(py).clear();
                    }
                }
            });
            for loader in self.loaders(py) {
                loader.reset(py);
            }
        }

        #[getter(get_template_cache)]
        fn get_template_cache<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
            let dict = self.with_loader(py, |loader| match loader {
                Loader::Cached(loader) => loader
                    .cache_view
                    .get_or_insert_with(|| PyDict::new(py).unbind())
                    .bind(py)
                    .clone(),
                _ => PyDict::new(py),
            });
            dict.clear();
            self.with_loader(py, |loader| -> PyResult<()> {
                if let Loader::Cached(loader) = loader {
                    for (name, entry) in &loader.cache {
                        match entry {
                            Ok(template) => {
                                let mut template = template.clone();
                                let mut path = self.path.clone();
                                path.extend(&template.loader_path);
                                template.loader_path = path;
                                dict.set_item(name, template)?;
                            }
                            Err(_) if self.engine.debug.load(Ordering::Relaxed) => {
                                dict.set_item(
                                    name,
                                    TemplateDoesNotExist::new_err(name.clone()).value(py),
                                )?;
                            }
                            Err(_) => {
                                dict.set_item(name, py.get_type::<TemplateDoesNotExist>())?;
                            }
                        }
                    }
                }
                Ok(())
            })?;
            Ok(dict)
        }

        #[pyo3(signature = (template_name, skip=None))]
        fn cache_key(
            &self,
            py: Python<'_>,
            template_name: Bound<'_, PyAny>,
            skip: Option<Bound<'_, PyAny>>,
        ) -> PyResult<String> {
            let name: String = template_name.str()?.extract()?;
            let mut matching = Vec::<String>::new();
            if let Some(skip) = skip {
                for origin in skip.try_iter()? {
                    let origin = origin?;
                    if origin.getattr("template_name")?.eq(&template_name)? {
                        matching.push(origin.getattr("name")?.extract()?);
                    }
                }
            }
            if matching.is_empty() {
                return Ok(name);
            }
            let bytes = pyo3::types::PyBytes::new(py, matching.join("|").as_bytes());
            let hash: String = py
                .import("hashlib")?
                .getattr("sha1")?
                .call1((bytes,))?
                .call_method0("hexdigest")?
                .extract()?;
            Ok(format!("{name}-{hash}"))
        }

        fn _cache_missing(&self, py: Python<'_>, key: String, tried: Vec<(String, String)>) {
            self.with_loader(py, |loader| {
                if let Loader::Cached(loader) = loader {
                    loader
                        .cache
                        .insert(key, Err(crate::loaders::LoaderError { tried }));
                }
            });
        }

        fn _cache_template(
            &self,
            py: Python<'_>,
            key: String,
            renderer: Bound<'_, PyAny>,
        ) -> PyResult<()> {
            let origin = renderer.getattr("origin")?;
            let origin_loader = origin.getattr("loader")?;
            let native_loader = origin_loader.getattr("_native").unwrap_or(origin_loader);
            let loader = native_loader.extract::<PyRef<'_, LoaderView>>()?;
            let template = if let Ok(native) = renderer.extract::<PyRef<'_, Template>>() {
                let mut template = native.clone();
                template.loader_path = loader.path[self.path.len()..].to_vec();
                template
            } else {
                Template {
                    filename: Some(PathBuf::from(origin.getattr("name")?.extract::<String>()?)),
                    template: Arc::from(renderer.getattr("source")?.extract::<String>()?),
                    nodes: Arc::new(Vec::new()),
                    has_inheritance: false,
                    has_python_callbacks: false,
                    engine: self.engine.clone(),
                    template_name: Some(renderer.getattr("name")?.extract()?),
                    loader_path: loader.path[self.path.len()..].to_vec(),
                    inspection_nodes: Arc::new(InspectionNodes(PyOnceLock::new())),
                    django_renderer: Arc::new(InspectionNodes(PyOnceLock::new())),
                    fallback_blocks: Arc::new(InspectionNodes(PyOnceLock::new())),
                    native_blocks: Arc::new(OnceLock::new()),
                    native_partials: Arc::new(OnceLock::new()),
                    partial_data: Arc::new(InspectionNodes(PyOnceLock::new())),
                    python_renderer: Some(Arc::new(renderer.unbind())),
                }
            };
            self.with_loader(py, |loader| {
                if let Loader::Cached(loader) = loader {
                    loader.cache.insert(key, Ok(template));
                }
            });
            self.get_template_cache(py)?;
            Ok(())
        }

        fn get_template(&self, py: Python<'_>, template_name: String) -> PyResult<Template> {
            let result = self.with_loader(py, |loader| {
                loader.get_template(py, &template_name, self.engine.clone())
            });
            match result {
                Ok(template) => template.map(|mut template| {
                    let mut path = self.path.clone();
                    path.extend(&template.loader_path);
                    template.loader_path = path;
                    template
                }),
                Err(_) => Err(TemplateDoesNotExist::new_err(template_name)),
            }
        }

        fn get_template_sources(
            &self,
            py: Python<'_>,
            template_name: Bound<'_, PyAny>,
        ) -> PyResult<Vec<Origin>> {
            if template_name.is_instance_of::<pyo3::types::PyBytes>() {
                return Err(PyTypeError::new_err(
                    "Can't mix strings and bytes in path components",
                ));
            }
            let name: String = template_name.str()?.extract()?;
            let children = self.loaders(py);
            if !children.is_empty() {
                let mut sources = Vec::new();
                for child in children {
                    sources.extend(child.get_template_sources(py, template_name.clone())?);
                }
                return Ok(sources);
            }
            if self.name(py) == "django.template.loaders.locmem.Loader" {
                return Ok(vec![Origin {
                    name: name.clone(),
                    template_name: Some(name),
                    loader: Some(self.clone()),
                }]);
            }
            let join = py.import("django.utils._os")?.getattr("safe_join")?;
            let mut origins = Vec::new();
            for directory in self.get_dirs(py)?.try_iter()? {
                let directory = directory?;
                match join.call1((directory, &name)) {
                    Ok(path) => origins.push(Origin {
                        name: path.extract()?,
                        template_name: Some(name.clone()),
                        loader: Some(self.clone()),
                    }),
                    Err(error)
                        if error.is_instance(
                            py,
                            &py.import("django.core.exceptions")?
                                .getattr("SuspiciousFileOperation")?,
                        ) => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(origins)
        }

        fn get_contents(&self, py: Python<'_>, origin: PyRef<'_, Origin>) -> PyResult<String> {
            if let Some(contents) = self.with_loader(py, |loader| match loader {
                Loader::LocMem(loader) => Some(loader.templates.get(&origin.name).cloned()),
                _ => None,
            }) {
                return contents.ok_or_else(|| TemplateDoesNotExist::new_err(origin.name.clone()));
            }
            let kwargs = PyDict::new(py);
            kwargs.set_item("encoding", &self.engine.file_charset)?;
            let file = py
                .import("builtins")?
                .getattr("open")?
                .call((&origin.name,), Some(&kwargs))
                .map_err(|error| {
                    if error.is_instance_of::<pyo3::exceptions::PyFileNotFoundError>(py) {
                        TemplateDoesNotExist::new_err(origin.name.clone())
                    } else {
                        error
                    }
                })?;
            let result = file.call_method0("read");
            file.call_method0("close")?;
            result?.extract()
        }
    }

    #[derive(Clone)]
    #[pyclass(frozen, skip_from_py_object)]
    pub(crate) struct Origin {
        #[pyo3(get)]
        name: String,
        #[pyo3(get)]
        template_name: Option<String>,
        #[pyo3(get)]
        loader: Option<LoaderView>,
    }

    impl Origin {
        pub(crate) fn matches(&self, other: &Bound<'_, PyAny>) -> bool {
            if let Ok(other) = other.extract::<PyRef<'_, Self>>() {
                return self.name == other.name && self.loader == other.loader;
            }
            other
                .getattr("_native_origin")
                .ok()
                .and_then(|origin| {
                    origin
                        .extract::<PyRef<'_, Self>>()
                        .ok()
                        .map(|other| self.name == other.name && self.loader == other.loader)
                })
                .unwrap_or(false)
        }
    }

    #[pymethods]
    impl Origin {
        fn __str__(&self) -> &str {
            &self.name
        }
        fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
            Ok(format!(
                "<Origin name={}>",
                PyString::new(py, &self.name).repr()?
            ))
        }
        fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
            self.matches(other)
        }
        #[getter]
        fn loader_name(&self, py: Python<'_>) -> Option<String> {
            self.loader
                .as_ref()
                .map(|loader| loader.name(py).to_string())
        }
    }

    fn import_libraries(libraries: Bound<'_, PyAny>) -> PyResult<HashMap<String, Py<PyAny>>> {
        let py = libraries.py();
        let libraries: HashMap<String, String> = libraries.extract()?;
        let mut libs = HashMap::with_capacity(libraries.len());
        for (name, path) in libraries {
            let library = match py.import(&path).ok_or_isinstance_of::<PyImportError>(py)? {
                Ok(library) => library,
                Err(e) => {
                    let error = format!(
                        "Invalid template library specified. ImportError raised when trying to load '{}': {}",
                        path,
                        e.value(py)
                    );
                    return Err(InvalidTemplateLibrary::new_err(error));
                }
            };
            let Ok(library) = library
                .getattr(intern!(py, "register"))
                .ok_or_isinstance_of::<PyAttributeError>(py)?
            else {
                let error = format!("Module '{path}' does not have a variable named 'register'");
                return Err(InvalidTemplateLibrary::new_err(error));
            };
            libs.insert(name, library.unbind());
        }
        Ok(libs)
    }

    /// Helper function to unpack a loader tuple configuration.
    /// See <https://docs.djangoproject.com/en/stable/ref/templates/api/#django.template.Engine>
    fn unpack<'py>(loader: &Bound<'py, PyAny>) -> PyResult<(String, Bound<'py, PyAny>)> {
        let mut items = loader.try_iter()?;
        let first_item = match items.next() {
            Some(item) => item?,
            None => return Err(ImproperlyConfigured::new_err("Configuration is empty")),
        };
        let loader_path = first_item.extract::<String>().map_err(|_| {
            ImproperlyConfigured::new_err(
                "First element of tuple configuration must be a Loader class name",
            )
        })?;
        let remaining_args = match items.next() {
            Some(item) => item?,
            None => {
                return Err(ImproperlyConfigured::new_err(
                    "Missing second element in tuple configuration",
                ));
            }
        };
        Ok((loader_path, remaining_args))
    }

    fn get_template_loaders(
        py: Python<'_>,
        template_loaders: Bound<'_, PyIterator>,
        encoding: &'static Encoding,
        dirs: &Vec<PathBuf>,
    ) -> PyResult<Vec<Loader>> {
        template_loaders
            .map(|template_loader| {
                template_loader.and_then(|template_loader| {
                    find_template_loader(py, template_loader, encoding, dirs)
                })
            })
            .collect()
    }

    fn find_template_loader(
        py: Python<'_>,
        loader: Bound<'_, PyAny>,
        encoding: &'static Encoding,
        dirs: &Vec<PathBuf>,
    ) -> PyResult<Loader> {
        if let Ok(loader_str) = loader.extract::<String>() {
            return map_loader(py, &loader_str, None, encoding, dirs);
        }

        let (loader_path, args) = unpack(&loader).map_err(|e| {
            ImproperlyConfigured::new_err(format!(
                "Invalid template loader: {loader}. {}",
                e.value(py),
            ))
        })?;

        map_loader(py, &loader_path, Some(args), encoding, dirs)
    }

    fn map_loader(
        py: Python<'_>,
        loader_path: &str,
        args: Option<Bound<'_, PyAny>>,
        encoding: &'static Encoding,
        dirs: &Vec<PathBuf>,
    ) -> PyResult<Loader> {
        match loader_path {
            "django.template.loaders.filesystem.Loader" => {
                let uses_engine_dirs = args.is_none();
                let paths = args
                    .map(|arg| {
                        arg.try_iter()?
                            .map(|item| item?.extract::<PathBuf>())
                            .collect::<PyResult<Vec<_>>>()
                    })
                    .transpose()?
                    .unwrap_or(dirs.clone());

                Ok(Loader::FileSystem(if uses_engine_dirs {
                    FileSystemLoader::with_engine_dirs(paths, encoding)
                } else {
                    FileSystemLoader::new(paths, encoding)
                }))
            }
            "django.template.loaders.app_directories.Loader" => {
                Ok(Loader::AppDirs(AppDirsLoader::new(encoding)))
            }
            "django.template.loaders.locmem.Loader" => {
                let templates = args
                    .map(|arg| arg.extract())
                    .transpose()?
                    .unwrap_or_default();

                Ok(Loader::LocMem(LocMemLoader::new(templates)))
            }
            "django.template.loaders.cached.Loader" => {
                let nested_loaders = args
                    .ok_or_else(|| {
                        ImproperlyConfigured::new_err(
                            "django.template.loaders.cached.Loader requires a list/tuple of loaders"
                        )
                    })?
                    .try_iter()?
                    .map(|inner_loader| find_template_loader(py, inner_loader?, encoding, dirs))
                    .collect::<PyResult<Vec<_>>>()?;

                Ok(Loader::Cached(CachedLoader::new(nested_loaders)))
            }
            // TODO: Return an `ExternalLoader` when it's fully implemented
            unknown => Err(ImproperlyConfigured::new_err(format!(
                "Invalid template loader class: {unknown}"
            ))),
        }
    }

    #[derive(Debug)]
    pub struct Engine {
        #[allow(dead_code)]
        dirs: Mutex<Vec<PathBuf>>,
        app_dirs: bool,
        context_processors: Vec<String>,
        debug: AtomicBool,
        template_loaders: Mutex<Vec<Loader>>,
        string_if_invalid: String,
        #[allow(dead_code)]
        encoding: &'static Encoding,
        pub file_charset: String,
        configuration: Option<Py<PyDict>>,
        builtins: Vec<String>,
        pub template_builtins: Vec<Py<PyAny>>,
        pub autoescape: bool,
        pub django_series: (u8, u8),
        loaded_context_processors: Vec<Py<PyAny>>,
        pub libraries: HashMap<String, Py<PyAny>>,
    }

    impl Engine {
        /// Peek only at the first loader: later loaders cannot bypass an
        /// earlier uncached loader's search order. Misses use normal loading.
        pub(crate) fn cached_native_template(
            &self,
            py: Python<'_>,
            name: &str,
        ) -> Option<Template> {
            let loaders = self
                .template_loaders
                .lock_py_attached(py)
                .expect("loader lock is not poisoned");
            let Loader::Cached(loader) = loaders.first()? else {
                return None;
            };
            let mut template = loader.cache.get(name)?.as_ref().ok()?.clone();
            if !template.is_native() {
                return None;
            }
            template.loader_path.insert(0, 0);
            Some(template)
        }

        pub fn dirs(&self, py: Python<'_>) -> PyResult<Vec<PathBuf>> {
            if let Some(configuration) = &self.configuration {
                return configuration
                    .bind(py)
                    .get_item("dirs")?
                    .expect("configured dirs")
                    .extract();
            }
            Ok(self
                .dirs
                .lock_py_attached(py)
                .expect("Mutex should not be poisoned")
                .clone())
        }

        #[cfg(test)]
        pub fn empty() -> Self {
            Self {
                dirs: Mutex::new(Vec::new()),
                app_dirs: false,
                context_processors: Vec::new(),
                debug: AtomicBool::new(false),
                template_loaders: Mutex::new(Vec::new()),
                string_if_invalid: String::new(),
                encoding: encoding_rs::UTF_8,
                file_charset: "utf-8".to_string(),
                configuration: None,
                builtins: Vec::new(),
                template_builtins: Vec::new(),
                autoescape: false,
                django_series: (5, 2),
                loaded_context_processors: Vec::new(),
                libraries: HashMap::new(),
            }
        }
    }

    pub fn get_template(
        engine: Arc<Engine>,
        py: Python<'_>,
        template_name: Cow<str>,
    ) -> PyResult<Template> {
        if template_name.contains('#') && engine.django_series >= (6, 0) {
            let (base, name) = template_name.split_once('#').expect("name contains #");
            let full = get_template(engine.clone(), py, Cow::Borrowed(base))?;
            if full.is_native() {
                if name.is_empty() {
                    return Ok(full);
                }
                let definition = full
                    .native_partials()
                    .get(name)
                    .ok_or_else(|| TemplateDoesNotExist::new_err(name.to_string()))?;
                return Ok(full.block_template(definition.nodes.clone()));
            }
            let renderer = py
                .import("django_rusty_templates.interop")?
                .getattr("compile_partial")?
                .call1((
                    template_name.as_ref(),
                    PyEngine {
                        engine: engine.clone(),
                    },
                ))?;
            return Ok(Template {
                filename: Some(PathBuf::from(
                    renderer
                        .getattr("origin")?
                        .getattr("name")?
                        .extract::<String>()?,
                )),
                template: Arc::from(renderer.getattr("source")?.extract::<String>()?),
                template_name: Some(template_name.into_owned()),
                loader_path: Vec::new(),
                inspection_nodes: Arc::new(InspectionNodes(PyOnceLock::new())),
                django_renderer: Arc::new(InspectionNodes(PyOnceLock::new())),
                fallback_blocks: Arc::new(InspectionNodes(PyOnceLock::new())),
                native_blocks: Arc::new(OnceLock::new()),
                native_partials: Arc::new(OnceLock::new()),
                partial_data: Arc::new(InspectionNodes(PyOnceLock::new())),
                python_renderer: Some(Arc::new(renderer.unbind())),
                nodes: Arc::new(Vec::new()),
                has_inheritance: false,
                has_python_callbacks: false,
                engine,
            });
        }
        let mut tried = Vec::new();
        let mut loaders = engine
            .template_loaders
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned");
        for (index, loader) in loaders.iter_mut().enumerate() {
            match loader.get_template(py, &template_name, engine.clone()) {
                Ok(template) => {
                    return template.and_then(|mut template| {
                        template.loader_path.insert(0, index);
                        if let Loader::Cached(cached) = loader
                            && let Some(view) = &cached.cache_view
                        {
                            view.bind(py)
                                .set_item(template_name.as_ref(), template.clone())?;
                        }
                        Ok(template)
                    });
                }
                Err(mut e) if !e.tried.is_empty() => tried.append(&mut e.tried),
                Err(_) => {}
            }
        }
        drop(loaders);
        let requested_name = template_name.to_string();
        Err(TemplateDoesNotExist::new_err((
            template_name.into_owned(),
            tried
                .into_iter()
                .map(|(name, reason)| {
                    (
                        Origin {
                            name,
                            template_name: Some(requested_name.clone()),
                            loader: None,
                        },
                        reason,
                    )
                })
                .collect::<Vec<_>>(),
        )))
    }

    pub fn select_template(
        engine: Arc<Engine>,
        py: Python<'_>,
        template_name_list: Vec<String>,
    ) -> PyResult<Template> {
        if template_name_list.is_empty() {
            return Err(TemplateDoesNotExist::new_err("No template names provided"));
        }
        let mut not_found = Vec::new();
        for template_name in template_name_list {
            match get_template(engine.clone(), py, Cow::Owned(template_name)) {
                Ok(template) => return Ok(template),
                Err(e) if e.is_instance_of::<TemplateDoesNotExist>(py) => {
                    not_found.push(e.value(py).to_string());
                }
                Err(e) => return Err(e),
            }
        }
        Err(TemplateDoesNotExist::new_err(not_found.join(", ")))
    }

    #[derive(Debug)]
    #[pyclass(name = "Engine", dict)]
    pub struct PyEngine {
        pub(crate) engine: Arc<Engine>,
    }

    #[pymethods]
    impl PyEngine {
        #[new]
        #[pyo3(signature = (dirs=None, app_dirs=false, context_processors=None, debug=false, loaders=None, string_if_invalid=String::new(), file_charset="utf-8".to_string(), libraries=None, builtins=None, autoescape=true))]
        #[allow(clippy::too_many_arguments)] // We're matching Django's Engine __init__ signature
        pub fn new(
            py: Python<'_>,
            dirs: Option<Bound<'_, PyAny>>,
            app_dirs: bool,
            context_processors: Option<Bound<'_, PyAny>>,
            debug: bool,
            loaders: Option<Bound<'_, PyAny>>,
            string_if_invalid: String,
            file_charset: String,
            libraries: Option<Bound<'_, PyAny>>,
            #[allow(unused_variables)] builtins: Option<Bound<'_, PyAny>>,
            autoescape: bool,
        ) -> PyResult<Self> {
            let configuration = PyDict::new(py);
            configuration.set_item(
                "dirs",
                dirs.as_ref()
                    .map_or_else(|| PyList::empty(py).into_any(), Clone::clone),
            )?;
            configuration.set_item(
                "context_processors",
                context_processors
                    .as_ref()
                    .map_or_else(|| PyList::empty(py).into_any(), Clone::clone),
            )?;
            configuration.set_item(
                "libraries",
                libraries
                    .as_ref()
                    .map_or_else(|| PyDict::new(py).into_any(), Clone::clone),
            )?;
            let builtin_names = PyList::new(
                py,
                [
                    "django.template.defaulttags",
                    "django.template.defaultfilters",
                    "django.template.loader_tags",
                ],
            )?;
            if let Some(builtins) = &builtins {
                for builtin in builtins.try_iter()? {
                    builtin_names.append(builtin?)?;
                }
            }
            configuration.set_item("builtins", builtin_names)?;
            let configured_loaders = match &loaders {
                Some(loaders) => loaders.clone(),
                None => {
                    let inner = PyList::new(py, ["django.template.loaders.filesystem.Loader"])?;
                    if app_dirs {
                        inner.append("django.template.loaders.app_directories.Loader")?;
                    }
                    let outer = PyList::empty(py);
                    outer.append(("django.template.loaders.cached.Loader", inner))?;
                    outer.into_any()
                }
            };
            configuration.set_item("loaders", configured_loaders)?;
            let dirs = match dirs {
                Some(dirs) => dirs.extract()?,
                None => Vec::new(),
            };

            let mut processors = vec!["django.template.context_processors.csrf".to_string()];
            if let Some(context_processors) = context_processors {
                processors.append(&mut context_processors.extract()?);
            }

            let import_string =
                IMPORT_STRING.import(py, "django.utils.module_loading", "import_string")?;
            let loaded_context_processors = processors
                .iter()
                .map(|name| import_string.call1((name,)).map(pyo3::Bound::unbind))
                .collect::<PyResult<Vec<_>>>()?;

            let encoding =
                Encoding::for_label(file_charset.as_bytes()).unwrap_or(encoding_rs::UTF_8);
            let template_loaders = match loaders {
                Some(_) if app_dirs => {
                    let err = ImproperlyConfigured::new_err(
                        "app_dirs must not be set when loaders is defined.",
                    );
                    return Err(err);
                }
                Some(loaders) => get_template_loaders(py, loaders.try_iter()?, encoding, &dirs)?,
                None => {
                    let filesystem_loader = Loader::FileSystem(FileSystemLoader::with_engine_dirs(
                        dirs.clone(),
                        encoding,
                    ));
                    let appdirs_loader = Loader::AppDirs(AppDirsLoader::new(encoding));
                    let loaders = if app_dirs {
                        vec![filesystem_loader, appdirs_loader]
                    } else {
                        vec![filesystem_loader]
                    };
                    let cached_loader = Loader::Cached(CachedLoader::new(loaders));
                    vec![cached_loader]
                }
            };
            let libraries = match libraries {
                None => HashMap::new(),
                Some(libraries) => import_libraries(libraries)?,
            };
            let builtins: Vec<String> = configuration
                .get_item("builtins")?
                .expect("configured builtins")
                .extract()?;
            let template_builtins = builtins
                .iter()
                .map(|module| {
                    py.import(module.as_str())?
                        .getattr("register")
                        .map(Bound::unbind)
                })
                .collect::<PyResult<Vec<_>>>()?;
            let version = py.import("django")?.getattr("VERSION")?;
            let django_series = (
                version.get_item(0)?.extract()?,
                version.get_item(1)?.extract()?,
            );
            let engine = Engine {
                django_series,
                dirs: Mutex::new(dirs),
                app_dirs,
                context_processors: processors,
                debug: AtomicBool::new(debug),
                template_loaders: template_loaders.into(),
                string_if_invalid,
                encoding,
                file_charset,
                configuration: Some(configuration.unbind()),
                builtins,
                template_builtins,
                autoescape,
                loaded_context_processors,
                libraries,
            };
            Ok(Self {
                engine: Arc::new(engine),
            })
        }

        /// Return a compiled Template object for the given template name,
        /// handling template inheritance recursively.
        ///
        /// See <https://docs.djangoproject.com/en/stable/ref/templates/api/#django.template.Engine.get_template>
        #[pyo3(signature = (template_name))]
        fn get_template<'py>(
            slf: Bound<'py, Self>,
            template_name: Option<Bound<'py, PyAny>>,
        ) -> PyResult<Bound<'py, PyAny>> {
            let py = slf.py();
            let template_name = template_name.unwrap_or_else(|| py.None().into_bound(py));
            if slf.getattr("__dict__")?.contains("find_template")?
                || slf.borrow().engine.django_series >= (6, 0)
            {
                return py
                    .import("django_rusty_templates")?
                    .getattr("_django_get_template")?
                    .call1((slf, template_name));
            }
            slf.borrow()
                .get_template_native(py, Some(template_name))?
                .into_pyobject(py)
                .map(Bound::into_any)
        }

        #[pyo3(signature = (template_name))]
        pub fn get_template_native(
            &self,
            py: Python<'_>,
            template_name: Option<Bound<'_, PyAny>>,
        ) -> PyResult<Template> {
            let template_name = template_name.unwrap_or_else(|| py.None().into_bound(py));
            let name = template_name.extract::<String>().ok();
            let Some(name) =
                name.filter(|name| !name.split('#').next().unwrap_or_default().is_empty())
            else {
                return Err(TemplateDoesNotExist::new_err((template_name.unbind(),)));
            };
            get_template(self.engine.clone(), py, Cow::Owned(name))
        }

        #[pyo3(signature = (name, dirs=None, skip=None))]
        fn find_template<'py>(
            slf: Bound<'py, Self>,
            name: Bound<'py, PyAny>,
            dirs: Option<Bound<'py, PyAny>>,
            skip: Option<Bound<'py, PyAny>>,
        ) -> PyResult<Bound<'py, PyAny>> {
            let _ = dirs;
            slf.py()
                .import("django_rusty_templates.interop")?
                .getattr("find_template")?
                .call1((slf, name, skip))
        }

        fn _compile_loaded_template<'py>(
            &self,
            py: Python<'py>,
            source: &str,
            origin: Bound<'py, PyAny>,
            name: &str,
        ) -> PyResult<Bound<'py, PyAny>> {
            let mut template = Template::new(
                py,
                source,
                PathBuf::from(origin.getattr("name")?.extract::<String>()?),
                name,
                self.engine.clone(),
            )?;
            let loader = origin.getattr("loader")?.getattr("_native")?;
            template.loader_path = loader.extract::<PyRef<'_, LoaderView>>()?.path.clone();
            if let Some(renderer) = &template.python_renderer {
                renderer.bind(py).call_method1("bind_origin", (origin,))?;
                return Ok(renderer.bind(py).clone());
            }
            template.into_pyobject(py).map(Bound::into_any)
        }

        fn select_template<'py>(
            slf: Bound<'py, Self>,
            names: Bound<'py, PyAny>,
        ) -> PyResult<Bound<'py, PyAny>> {
            let py = slf.py();
            if slf.borrow().engine.django_series >= (6, 0) {
                let template = py
                    .import("django_rusty_templates")?
                    .getattr("_django_select_template")?
                    .call1((slf, names))?;
                if let Ok(native) = template.extract::<PyRef<'_, Template>>() {
                    return native._django_template(py);
                }
                return Ok(template);
            }
            slf.borrow()
                .select_template_native(py, names.extract()?)?
                .into_pyobject(py)
                .map(Bound::into_any)
        }

        /// Given a list of template names, return the first that can be loaded.
        ///
        /// See <https://docs.djangoproject.com/en/stable/ref/templates/api/#django.template.Engine.select_template>
        #[pyo3(name = "_select_template_native")]
        pub fn select_template_native(
            &self,
            py: Python<'_>,
            template_name_list: Vec<String>,
        ) -> PyResult<Template> {
            select_template(self.engine.clone(), py, template_name_list)
        }

        /// Inspect native compilation without compiling a Python fallback tree.
        /// Custom tag libraries can execute registration code during inspection.
        #[pyo3(signature = (source, template_name=None))]
        fn explain_template<'py>(
            &self,
            py: Python<'py>,
            source: &str,
            template_name: Option<String>,
        ) -> PyResult<Bound<'py, PyDict>> {
            let report = PyDict::new(py);
            report.set_item("mode", "native")?;
            for key in ["reason", "tag", "span", "line"] {
                report.set_item(key, py.None())?;
            }
            let mut parser = Parser::new(
                py,
                TemplateString(source),
                self.engine.clone(),
                template_name.as_deref(),
            );
            parser.emit_warnings = false;
            parser.inspect_only = true;
            match parser.parse() {
                Ok(_) => {}
                Err(crate::parse::PyParseError::PyErr(error))
                    if error.is_instance_of::<PyNotImplementedError>(py) =>
                {
                    report.set_item(
                        "mode",
                        if can_compile_python_fallback(py, &error) {
                            "hybrid"
                        } else {
                            "unsupported"
                        },
                    )?;
                    report.set_item("reason", error.value(py).to_string())?;
                    if let Some(at) = parser.last_tag {
                        let token = TemplateString(source).content(at);
                        report.set_item("tag", token)?;
                        report.set_item("span", (at.0, at.1))?;
                        report.set_item(
                            "line",
                            source[..at.0].bytes().filter(|b| *b == b'\n').count() + 1,
                        )?;
                    }
                }
                Err(error) => {
                    return Err(canonical_parse_error(
                        py,
                        error,
                        self.engine.clone(),
                        source,
                        None,
                        None,
                    ));
                }
            }
            Ok(report)
        }

        #[pyo3(signature = (source, callbacks, template_name=None, filename=None))]
        fn _compile_interop_region(
            &self,
            py: Python<'_>,
            source: String,
            callbacks: Vec<(usize, usize, Py<PyAny>)>,
            template_name: Option<String>,
            filename: Option<PathBuf>,
        ) -> PyResult<Template> {
            let mut parser = Parser::new(
                py,
                TemplateString(&source),
                self.engine.clone(),
                template_name.as_deref(),
            );
            for (start, end, node) in callbacks {
                if start >= end
                    || end > source.len()
                    || !source.is_char_boundary(start)
                    || !source.is_char_boundary(end)
                {
                    return Err(PyValueError::new_err("Invalid interop node source span"));
                }
                parser
                    .python_nodes
                    .insert(start, (end, crate::parse::PythonNode(Arc::new(node))));
            }
            let nodes = parser.parse().map_err(|error| {
                if parser.compiler_parser.as_ref().is_some_and(|compiler| {
                    compiler
                        .bind(py)
                        .getattr("compiler_failed")
                        .is_ok_and(|failed| failed.extract::<bool>().unwrap_or(true))
                }) {
                    return native_parse_error(error, &source, filename.as_deref());
                }
                canonical_parse_error(
                    py,
                    error,
                    self.engine.clone(),
                    &source,
                    filename.as_deref(),
                    template_name.as_deref(),
                )
            })?;
            let has_python_callbacks =
                parser.compiler_parser.is_some() || !parser.python_nodes.is_empty();
            Ok(Template {
                template: Arc::from(source),
                filename,
                template_name,
                nodes: Arc::new(nodes),
                engine: self.engine.clone(),
                loader_path: Vec::new(),
                inspection_nodes: Arc::new(InspectionNodes(PyOnceLock::new())),
                django_renderer: Arc::new(InspectionNodes(PyOnceLock::new())),
                fallback_blocks: Arc::new(InspectionNodes(PyOnceLock::new())),
                native_blocks: Arc::new(OnceLock::new()),
                native_partials: Arc::new(OnceLock::new()),
                partial_data: Arc::new(InspectionNodes(PyOnceLock::new())),
                python_renderer: None,
                has_inheritance: false,
                has_python_callbacks,
            })
        }

        fn _syntax_diagnostic(
            &self,
            py: Python<'_>,
            source: &str,
            filename: Option<PathBuf>,
            template_name: Option<String>,
        ) -> PyResult<Option<String>> {
            let mut parser = Parser::new(
                py,
                TemplateString(source),
                self.engine.clone(),
                template_name.as_deref(),
            );
            parser.emit_warnings = false;
            parser.inspect_only = true;
            let parsed = parser.parse();
            match parsed {
                Ok(_) => Ok(None),
                Err(crate::parse::PyParseError::PyErr(error))
                    if error.is_instance_of::<PyNotImplementedError>(py) =>
                {
                    Ok(None)
                }
                Err(error) => {
                    // Django already rejected this source in the compatibility
                    // parser. Attach the native explanation without validating
                    // again or invoking arbitrary compilers a second time.
                    let error = native_parse_error(error, source, filename.as_deref());
                    Ok(Some(error.value(py).to_string()))
                }
            }
        }

        #[allow(clippy::wrong_self_convention)] // We're implementing a Django interface
        pub fn from_string(&self, template_code: Bound<'_, PyString>) -> PyResult<Template> {
            Template::new_from_string(
                template_code.py(),
                template_code.extract()?,
                self.engine.clone(),
            )
        }

        /// Preserve Django's direct Template constructor metadata in conformance tests.
        fn _from_string(
            &self,
            py: Python<'_>,
            source: &str,
            filename: Option<PathBuf>,
            template_name: Option<String>,
        ) -> PyResult<Template> {
            Template::compile(py, source, filename, template_name, self.engine.clone())
        }

        /// Render the template specified by `template_name` with the given context.
        /// For use in Django's test suite.
        #[pyo3(signature = (template_name, context=None))]
        pub fn render_to_string(
            &self,
            py: Python<'_>,
            template_name: Bound<'_, PyAny>,
            context: Option<Bound<'_, PyAny>>,
        ) -> PyResult<String> {
            let template = if template_name.is_instance_of::<PyList>()
                || template_name.is_instance_of::<PyTuple>()
            {
                self.select_template_native(py, template_name.extract()?)?
            } else {
                get_template(
                    self.engine.clone(),
                    py,
                    Cow::Owned(template_name.extract::<String>()?),
                )?
            };

            template.py_render(py, context, None)
        }

        fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
            let configuration = self
                .engine
                .configuration
                .as_ref()
                .expect("Python engines have configuration")
                .bind(py);
            let mut parts = vec!["<Engine:".to_string()];
            let dirs = configuration.get_item("dirs")?.expect("configured dirs");
            if dirs.is_truthy()? {
                parts.push(format!("dirs={}", dirs.repr()?));
            }
            parts.push(format!(
                "app_dirs={}",
                if self.engine.app_dirs {
                    "True"
                } else {
                    "False"
                }
            ));
            let processors = configuration
                .get_item("context_processors")?
                .expect("configured processors");
            if processors.is_truthy()? {
                parts.push(format!("context_processors={}", processors.repr()?));
            }
            parts.push(format!(
                "debug={}",
                if self.debug() { "True" } else { "False" }
            ));
            parts.push(format!(
                "loaders={}",
                configuration
                    .get_item("loaders")?
                    .expect("configured loaders")
                    .repr()?
            ));
            parts.push(format!(
                "string_if_invalid={}",
                PyString::new(py, &self.engine.string_if_invalid).repr()?
            ));
            parts.push(format!(
                "file_charset={}",
                PyString::new(py, &self.engine.file_charset).repr()?
            ));
            for name in ["libraries", "builtins"] {
                let value = configuration.get_item(name)?.expect("configured option");
                if value.is_truthy()? {
                    parts.push(format!("{name}={}", value.repr()?));
                }
            }
            parts.push(format!(
                "autoescape={}>",
                if self.engine.autoescape {
                    "True"
                } else {
                    "False"
                }
            ));
            Ok(parts.join(" "))
        }

        #[getter]
        pub fn dirs<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
            self.engine
                .configuration
                .as_ref()
                .expect("Python engines have configuration")
                .bind(py)
                .get_item("dirs")?
                .ok_or_else(|| PyAttributeError::new_err("dirs"))
        }

        #[setter]
        pub fn set_dirs(&self, py: Python<'_>, dirs: Bound<'_, PyAny>) -> PyResult<()> {
            *self
                .engine
                .dirs
                .lock_py_attached(py)
                .expect("Mutex should not be poisoned") = dirs.extract()?;
            self.engine
                .configuration
                .as_ref()
                .expect("Python engines have configuration")
                .bind(py)
                .set_item("dirs", dirs)
        }

        #[getter]
        pub fn app_dirs(&self) -> bool {
            self.engine.app_dirs
        }

        #[getter]
        pub fn context_processors(&self) -> &Vec<String> {
            &self.engine.context_processors
        }

        #[getter]
        pub fn debug(&self) -> bool {
            self.engine.debug.load(Ordering::Relaxed)
        }

        #[setter]
        pub fn set_debug(&self, debug: bool) {
            self.engine.debug.store(debug, Ordering::Relaxed);
        }

        #[getter]
        fn template_loaders(slf: Bound<'_, Self>) -> PyResult<Bound<'_, PyList>> {
            let py = slf.py();
            let dict = slf.getattr("__dict__")?;
            if let Ok(loaders) = dict.get_item("_template_loaders") {
                return Ok(loaders.cast_into::<PyList>()?);
            }
            let engine = slf.borrow().engine.clone();
            let count = engine
                .template_loaders
                .lock_py_attached(py)
                .expect("Mutex should not be poisoned")
                .len();
            let wrap = py
                .import("django_rusty_templates.loaders")?
                .getattr("wrap_loader")?;
            let loaders = PyList::empty(py);
            for index in 0..count {
                loaders.append(wrap.call1((LoaderView {
                    engine: engine.clone(),
                    path: vec![index],
                },))?)?;
            }
            dict.set_item("_template_loaders", &loaders)?;
            Ok(loaders)
        }

        #[getter]
        pub fn template_context_processors<'py>(
            &self,
            py: Python<'py>,
        ) -> PyResult<Bound<'py, PyTuple>> {
            PyTuple::new(
                py,
                self.engine
                    .loaded_context_processors
                    .iter()
                    .map(|processor| processor.bind(py)),
            )
        }

        #[getter]
        pub fn string_if_invalid(&self) -> &str {
            &self.engine.string_if_invalid
        }

        #[getter]
        pub fn file_charset(&self) -> String {
            self.engine.file_charset.clone()
        }

        #[getter]
        pub fn builtins(&self) -> &Vec<String> {
            &self.engine.builtins
        }

        #[getter]
        pub fn template_builtins<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
            PyList::new(
                py,
                self.engine
                    .template_builtins
                    .iter()
                    .map(|library| library.bind(py)),
            )
        }

        #[getter]
        pub fn template_libraries<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
            let dict = PyDict::new(py);
            for (key, value) in &self.engine.libraries {
                dict.set_item(key, value.bind(py))?;
            }
            Ok(dict)
        }

        #[getter]
        pub fn autoescape(&self) -> bool {
            self.engine.autoescape
        }

        #[getter]
        pub fn libraries<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
            if let Some(configuration) = &self.engine.configuration {
                return Ok(configuration
                    .bind(py)
                    .get_item("libraries")?
                    .expect("configured libraries")
                    .cast_into::<PyDict>()?);
            }
            let dict = PyDict::new(py);
            for (key, value) in &self.engine.libraries {
                dict.set_item(key, value.bind(py))?;
            }
            Ok(dict)
        }
    }

    struct InspectionNodes(PyOnceLock<Py<PyAny>>);

    impl std::fmt::Debug for InspectionNodes {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("InspectionNodes")
        }
    }

    #[derive(Debug, Clone)]
    #[pyclass(skip_from_py_object)]
    pub struct Template {
        pub filename: Option<PathBuf>,
        pub template: Arc<str>,
        pub nodes: Arc<Vec<TokenTree>>,
        pub engine: Arc<Engine>,
        pub template_name: Option<String>,
        pub loader_path: Vec<usize>,
        inspection_nodes: Arc<InspectionNodes>,
        django_renderer: Arc<InspectionNodes>,
        fallback_blocks: Arc<InspectionNodes>,
        native_blocks: Arc<OnceLock<HashMap<String, Arc<Vec<TokenTree>>>>>,
        native_partials: Arc<OnceLock<HashMap<String, PartialDefinition>>>,
        partial_data: Arc<InspectionNodes>,
        has_inheritance: bool,
        has_python_callbacks: bool,
        python_renderer: Option<Arc<Py<PyAny>>>,
    }

    fn can_compile_python_fallback(py: Python<'_>, error: &PyErr) -> bool {
        [
            "block",
            "extends",
            "Fully custom tag",
            "partialdef",
            "partial",
        ]
        .iter()
        .any(|tag| error.value(py).to_string() == format!("not yet implemented: {tag}"))
    }

    fn native_parse_error(
        error: crate::parse::PyParseError,
        source: &str,
        filename: Option<&Path>,
    ) -> PyErr {
        match error.try_into_parse_error() {
            Ok(error) => {
                if let Some(filename) = filename {
                    TemplateSyntaxError::with_source_code(
                        error.into(),
                        miette::NamedSource::new(filename.to_string_lossy(), source.to_string()),
                    )
                } else {
                    TemplateSyntaxError::with_source_code(error.into(), source.to_string())
                }
            }
            Err(error) => error,
        }
    }

    fn canonical_parse_error(
        py: Python<'_>,
        error: crate::parse::PyParseError,
        engine: Arc<Engine>,
        source: &str,
        filename: Option<&Path>,
        template_name: Option<&str>,
    ) -> PyErr {
        if let crate::parse::PyParseError::PyErr(ref error) = error
            && (!error.is_instance_of::<PyException>(py)
                || error
                    .value(py)
                    .hasattr("_rusty_compiler_error")
                    .unwrap_or(false))
        {
            return error.clone_ref(py);
        }
        let native = native_parse_error(error, source, filename);
        let validation = (|| -> PyResult<()> {
            py.import("django_rusty_templates.diagnostics")?
                .getattr("validate_syntax")?
                .call1((
                    source,
                    PyEngine { engine },
                    filename.map(|filename| filename.to_string_lossy().into_owned()),
                    template_name,
                ))?;
            Ok(())
        })();
        match validation {
            Err(error) => {
                if !error.is_instance_of::<PyException>(py) {
                    return error;
                }
                let diagnostic = native.value(py).to_string();
                let _ = error.value(py).setattr("rusty_diagnostic", &diagnostic);
                let _ = error.value(py).call_method1("add_note", (diagnostic,));
                error
            }
            Ok(()) => native,
        }
    }

    impl Template {
        fn compile(
            py: Python<'_>,
            template: &str,
            filename: Option<PathBuf>,
            template_name: Option<String>,
            engine: Arc<Engine>,
        ) -> PyResult<Self> {
            let mut parser = Parser::new(
                py,
                TemplateString(template),
                engine.clone(),
                template_name.as_deref(),
            );
            parser.warning_origin = filename
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned());
            let mut python_renderer = None;
            let parsed = parser.parse();
            let nodes = match parsed {
                Ok(nodes) => nodes,
                Err(crate::parse::PyParseError::PyErr(error))
                    if error.is_instance_of::<PyNotImplementedError>(py)
                        && can_compile_python_fallback(py, &error) =>
                {
                    python_renderer = Some(Arc::new(
                        py.import("django_rusty_templates.interop")?
                            .getattr("compile_template")?
                            .call1((
                                template,
                                PyEngine {
                                    engine: engine.clone(),
                                },
                                filename
                                    .as_ref()
                                    .map(|path| path.to_string_lossy().into_owned()),
                                template_name.as_deref(),
                            ))?
                            .unbind(),
                    ));
                    Vec::new()
                }
                Err(error) => {
                    if let Some(compiler) = &parser.compiler_parser {
                        let native = native_parse_error(error, template, filename.as_deref());
                        if !native.is_instance_of::<PyException>(py)
                            || compiler
                                .bind(py)
                                .getattr("compiler_failed")?
                                .extract::<bool>()?
                            || native.value(py).hasattr("_rusty_compiler_error")?
                        {
                            return Err(native);
                        }
                        return Err(PyErr::from_value(
                            compiler
                                .bind(py)
                                .call_method1("canonical_error", (native.value(py),))?,
                        ));
                    }
                    return Err(canonical_parse_error(
                        py,
                        error,
                        engine.clone(),
                        template,
                        filename.as_deref(),
                        template_name.as_deref(),
                    ));
                }
            };
            let partial_data = Arc::new(InspectionNodes(PyOnceLock::new()));
            if let Some(compiler) = &parser.compiler_parser {
                partial_data.0.get_or_try_init(py, || {
                    compiler.bind(py).getattr("extra_data").map(Bound::unbind)
                })?;
            }
            let has_python_callbacks =
                parser.compiler_parser.is_some() || !parser.python_nodes.is_empty();
            Ok(Self {
                template: Arc::from(template),
                filename,
                template_name,
                loader_path: Vec::new(),
                inspection_nodes: Arc::new(InspectionNodes(PyOnceLock::new())),
                django_renderer: Arc::new(InspectionNodes(PyOnceLock::new())),
                fallback_blocks: Arc::new(InspectionNodes(PyOnceLock::new())),
                native_blocks: Arc::new(OnceLock::new()),
                native_partials: Arc::new(OnceLock::new()),
                partial_data,
                python_renderer,
                has_inheritance: Self::nodes_have_inheritance(&nodes),
                has_python_callbacks,
                nodes: Arc::new(nodes),
                engine,
            })
        }

        pub fn new(
            py: Python<'_>,
            template: &str,
            filename: PathBuf,
            template_name: &str,
            engine: Arc<Engine>,
        ) -> PyResult<Self> {
            Self::compile(
                py,
                template,
                Some(filename),
                Some(template_name.to_string()),
                engine,
            )
        }

        pub fn new_from_string(
            py: Python<'_>,
            template: String,
            engine: Arc<Engine>,
        ) -> PyResult<Self> {
            Self::compile(py, &template, None, None, engine)
        }

        pub fn render(&self, py: Python<'_>, context: &mut Context) -> RenderResult<'_> {
            self.with_render_scope(py, context, |context| self.render_chain(py, context))
        }

        pub(crate) fn render_into(
            &self,
            py: Python<'_>,
            context: &mut Context,
            output: &mut String,
        ) -> crate::render::RenderIntoResult {
            self.with_render_scope(py, context, |context| {
                self.render_chain_into(py, context, output)
            })
        }

        fn with_render_scope<T>(
            &self,
            py: Python<'_>,
            context: &mut Context,
            render: impl FnOnce(&mut Context) -> Result<T, PyRenderError>,
        ) -> Result<T, PyRenderError> {
            let previous_blocks = std::mem::take(&mut context.inheritance_blocks);
            let previous_history = std::mem::take(&mut context.inheritance_history);
            let previous_root = context.root_template.clone();
            let previous_cycles = std::mem::take(&mut context.cycle_indices);
            let previous_instrument = std::mem::replace(&mut context.instrument_render, true);
            let callback_scope = if self.has_python_callbacks {
                context
                    .formatting_context
                    .as_ref()
                    .map(|live| {
                        py.import("django_rusty_templates.compiler")?
                            .getattr("begin_render")?
                            .call1((live.bind(py), Py::new(py, self.clone())?))
                    })
                    .transpose()?
            } else {
                None
            };
            let result = render(context);
            if let Some(scope) = callback_scope {
                let cleanup = scope.call_method0("close");
                if result.is_ok() {
                    cleanup?;
                }
            }
            context.inheritance_blocks = previous_blocks;
            context.inheritance_history = previous_history;
            context.root_template = previous_root;
            context.cycle_indices = previous_cycles;
            context.instrument_render = previous_instrument;
            result
        }

        pub(crate) fn is_native(&self) -> bool {
            self.python_renderer.is_none()
        }

        pub(crate) fn extends_parent(&self) -> bool {
            self.nodes
                .iter()
                .any(|node| matches!(node, TokenTree::Tag(Tag::Extends { .. })))
        }

        pub(crate) fn native_partials(&self) -> &HashMap<String, PartialDefinition> {
            self.native_partials.get_or_init(|| {
                fn collect(nodes: &[TokenTree], partials: &mut HashMap<String, PartialDefinition>) {
                    for node in nodes {
                        if let TokenTree::Tag(Tag::PartialDef { definition, .. }) = node {
                            partials.insert(definition.name.clone(), definition.clone());
                        }
                        for children in node.child_nodes() {
                            collect(children, partials);
                        }
                    }
                }
                let mut partials = HashMap::new();
                collect(&self.nodes, &mut partials);
                partials
            })
        }

        pub(crate) fn native_blocks(&self) -> &HashMap<String, Arc<Vec<TokenTree>>> {
            self.native_blocks.get_or_init(|| {
                fn collect(nodes: &[TokenTree], blocks: &mut HashMap<String, Arc<Vec<TokenTree>>>) {
                    for node in nodes {
                        if let TokenTree::Tag(Tag::Block { name, nodes }) = node {
                            blocks.insert(name.clone(), nodes.clone());
                        }
                        for children in node.child_nodes() {
                            collect(children, blocks);
                        }
                    }
                }
                let mut blocks = HashMap::new();
                collect(&self.nodes, &mut blocks);
                blocks
            })
        }

        pub(crate) fn block_template(&self, nodes: Arc<Vec<TokenTree>>) -> Self {
            // A detached body must resolve definitions from the complete owner,
            // including definitions outside an inherited block.
            self.native_partials();
            Self {
                has_inheritance: Self::nodes_have_inheritance(&nodes),
                has_python_callbacks: self.has_python_callbacks
                    && Self::nodes_have_callbacks(&nodes),
                nodes,
                inspection_nodes: Arc::new(InspectionNodes(PyOnceLock::new())),
                django_renderer: Arc::new(InspectionNodes(PyOnceLock::new())),
                fallback_blocks: Arc::new(InspectionNodes(PyOnceLock::new())),
                native_blocks: Arc::new(OnceLock::new()),
                native_partials: self.native_partials.clone(),
                partial_data: Arc::new(InspectionNodes(PyOnceLock::new())),
                ..self.clone()
            }
        }

        fn has_inheritance(&self) -> bool {
            self.has_inheritance
        }

        fn nodes_have_callbacks(nodes: &[TokenTree]) -> bool {
            nodes.iter().any(|node| {
                matches!(node, TokenTree::Tag(Tag::PythonNode(_)))
                    || node
                        .child_nodes()
                        .iter()
                        .any(|children| Self::nodes_have_callbacks(children))
            })
        }

        fn nodes_have_inheritance(nodes: &[TokenTree]) -> bool {
            fn contains(nodes: &[TokenTree]) -> bool {
                nodes.iter().any(|node| {
                    matches!(
                        node,
                        TokenTree::Tag(Tag::Block { .. } | Tag::Extends { .. })
                    ) || node.child_nodes().iter().any(|nodes| contains(nodes))
                })
            }
            contains(nodes)
        }

        pub(crate) fn compiled_blocks<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
            self.fallback_blocks
                .0
                .get_or_try_init(py, || {
                    fn collect(
                        py: Python<'_>,
                        owner: &Template,
                        nodes: &[TokenTree],
                        blocks: &Bound<'_, PyDict>,
                    ) -> PyResult<()> {
                        for node in nodes {
                            if let TokenTree::Tag(Tag::Block { name, nodes }) = node {
                                let body = owner.block_template(nodes.clone());
                                let block = py
                                    .import("django_rusty_templates.inheritance")?
                                    .getattr("make_block")?
                                    .call1((name, Py::new(py, body)?))?;
                                blocks.set_item(name, block)?;
                            }
                            for children in node.child_nodes() {
                                collect(py, owner, children, blocks)?;
                            }
                        }
                        Ok(())
                    }
                    let blocks = PyDict::new(py);
                    collect(py, self, &self.nodes, &blocks)?;
                    Ok(blocks.into_any().unbind())
                })
                .map(|blocks| blocks.bind(py).clone())
        }

        pub(crate) fn render_chain(
            &self,
            py: Python<'_>,
            context: &mut Context,
        ) -> RenderResult<'_> {
            if let Some(renderer) = &self.python_renderer {
                renderer
                    .bind(py)
                    .call_method1("bind_origin", (self.origin(),))?;
                let output: String = renderer
                    .bind(py)
                    .call_method1("render", (context.django_context(py)?,))?
                    .extract()?;
                return Ok(Cow::Owned(output));
            }
            let mut output = String::with_capacity(self.template.len());
            self.render_chain_into(py, context, &mut output)?;
            Ok(Cow::Owned(output))
        }

        pub(crate) fn render_chain_into(
            &self,
            py: Python<'_>,
            context: &mut Context,
            output: &mut String,
        ) -> crate::render::RenderIntoResult {
            if let Some(renderer) = &self.python_renderer {
                renderer
                    .bind(py)
                    .call_method1("bind_origin", (self.origin(),))?;
                let rendered = renderer
                    .bind(py)
                    .call_method1("render", (context.django_context(py)?,))?;
                crate::render::types::append_python_string(&rendered, false, output)?;
                return Ok(());
            }
            // Django fallbacks already signal via Template._render. Native regions
            // inside those templates are implementation details, not templates.
            if context.instrument_render {
                let signal =
                    TEMPLATE_RENDERED.import(py, "django.test.signals", "template_rendered")?;
                // Signal.has_listeners() takes a lock and inspects receivers even
                // when its list is empty. Most production renders have none.
                if signal.getattr("receivers")?.is_truthy()?
                    && signal.call_method0("has_listeners")?.is_truthy()?
                {
                    let template = Py::new(py, self.clone())?;
                    let kwargs = PyDict::new(py);
                    kwargs.set_item("sender", &template)?;
                    kwargs.set_item("template", &template)?;
                    kwargs.set_item("context", context.django_context(py)?)?;
                    signal.call_method("send", (), Some(&kwargs))?;
                }
            }
            if context.formatting_context.is_none() && self.has_python_callbacks {
                let direct_input =
                    context.current_template.is_none() && context.callback_input.is_some();
                let live = if direct_input {
                    context
                        .callback_input
                        .as_ref()
                        .expect("direct input requires a caller")
                        .bind(py)
                        .call0()?
                } else {
                    context.django_context(py)?
                };
                let processor_values = if direct_input
                    && live
                        .getattr("_processors_index")
                        .is_ok_and(|index| !index.is_none())
                {
                    Some(context.as_dict(py)?)
                } else {
                    None
                };
                let scope = py
                    .import("django_rusty_templates.compiler")?
                    .getattr("begin_render")?
                    .call1((&live, Py::new(py, self.clone())?, processor_values))?;
                let before: Option<HashMap<String, Py<PyAny>>> = if direct_input {
                    None
                } else {
                    Some(live.call_method0("flatten")?.extract()?)
                };
                context.formatting_context = Some(live.unbind());
                let result = self.render_body_into(py, &self.nodes, context, output);
                let live = context
                    .formatting_context
                    .take()
                    .expect("callback context initialized");
                let cleanup = scope.call_method0("close");
                if result.is_ok() {
                    cleanup?;
                }
                if let Some(before) = before {
                    let after: HashMap<String, Py<PyAny>> =
                        live.bind(py).call_method0("flatten")?.extract()?;
                    for (name, value) in after {
                        if before
                            .get(&name)
                            .is_none_or(|previous| !previous.bind(py).is(value.bind(py)))
                        {
                            context.insert(name, value.into_bound(py))?;
                        }
                    }
                } else {
                    context.refresh_assignments(py, live.bind(py))?;
                }
                return result;
            }
            self.render_body_into(py, &self.nodes, context, output)
        }

        pub(crate) fn render_body<'a>(
            &'a self,
            py: Python<'_>,
            nodes: &[TokenTree],
            context: &mut Context,
        ) -> RenderResult<'a> {
            let mut output = String::with_capacity(self.template.len());
            self.render_body_into(py, nodes, context, &mut output)?;
            Ok(Cow::Owned(output))
        }

        pub(crate) fn render_body_into(
            &self,
            py: Python<'_>,
            nodes: &[TokenTree],
            context: &mut Context,
            output: &mut String,
        ) -> crate::render::RenderIntoResult {
            let python_blocks = if self.has_inheritance() && context.formatting_context.is_some() {
                Some(self.compiled_blocks(py)?.unbind())
            } else {
                None
            };
            let previous_python_blocks =
                std::mem::replace(&mut context.python_blocks, python_blocks);
            let owner = Arc::new(self.clone());
            if context.root_template.is_none() {
                context.root_template = Some(owner.clone());
            }
            let previous_template = context.current_template.replace(owner);
            let previous_invalid = std::mem::replace(
                &mut context.string_if_invalid,
                self.engine.string_if_invalid.clone(),
            );
            let result = (|| {
                let template = TemplateString(&self.template);
                let mut join = crate::render::JoinResults::default();
                for (index, node) in nodes.iter().enumerate() {
                    match node.render_into(py, template, context, output) {
                        Err(PyRenderError::InvalidTagResult(value)) => join.push(index, value),
                        Err(PyRenderError::PyErr(error)) => {
                            if self.engine.debug.load(Ordering::Relaxed) {
                                let _ = (|| -> PyResult<()> {
                                    py.import("django_rusty_templates.diagnostics")?
                                        .getattr("attach_debug")?
                                        .call1((
                                            self.template.as_ref(),
                                            error.value(py),
                                            self.filename
                                                .as_ref()
                                                .map(|path| path.to_string_lossy().into_owned()),
                                        ))?;
                                    Ok(())
                                })();
                            }
                            return Err(error.into());
                        }
                        Err(error) => return Err(self.render_error(error).into()),
                        Ok(()) => {}
                    }
                }
                join.finish(py)
            })();
            context.string_if_invalid = previous_invalid;
            context.python_blocks = previous_python_blocks;
            context.current_template = previous_template;
            result
        }

        fn render_django_context(
            &self,
            py: Python<'_>,
            context: Bound<'_, PyAny>,
            instrument_render: bool,
        ) -> PyResult<String> {
            let mut rendering_context = Context::new(
                HashMap::new(),
                context.getattr("request").ok().map(Bound::unbind),
                context.getattr("autoescape")?.extract()?,
            );
            rendering_context.use_l10n = context.getattr("use_l10n")?.extract()?;
            rendering_context.use_tz = context.getattr("use_tz")?.extract()?;
            rendering_context.format_values = true;
            rendering_context.formatting_context = Some(context.unbind());
            rendering_context.instrument_render = instrument_render;
            self._render(py, &mut rendering_context)
        }

        fn _render(&self, py: Python<'_>, context: &mut Context) -> PyResult<String> {
            self.render_chain(py, context)
                .map(Cow::into_owned)
                .map_err(|error| self.render_error(error))
        }

        pub(crate) fn render_error(&self, error: PyRenderError) -> PyErr {
            let error = match error.try_into_render_error() {
                Ok(error) => error,
                Err(error) => return error,
            };
            let span = miette::Diagnostic::labels(&error)
                .and_then(|mut labels| labels.next())
                .map(|label| (label.offset(), label.len()));
            let error = match error {
                RenderError::RelativePathError(_) => {
                    TemplateSyntaxError::with_source_code(error.into(), self.template.to_string())
                }
                RenderError::VariableDoesNotExist { .. }
                | RenderError::ArgumentDoesNotExist { .. } => {
                    VariableDoesNotExist::with_source_code(error.into(), self.template.to_string())
                }
                RenderError::InvalidArgumentInteger { .. }
                | RenderError::InvalidArgumentString { .. }
                | RenderError::TupleUnpackError { .. } => {
                    PyValueError::with_source_code(error.into(), self.template.to_string())
                }
                RenderError::OverflowError { .. } | RenderError::InvalidArgumentFloat { .. } => {
                    PyOverflowError::with_source_code(error.into(), self.template.to_string())
                }
            };
            if self.engine.debug.load(Ordering::Relaxed)
                && let Some(span) = span
            {
                Python::attach(|py| {
                    let _ = (|| -> PyResult<()> {
                        error.value(py).setattr("_rusty_token_span", span)?;
                        py.import("django_rusty_templates.diagnostics")?
                            .getattr("attach_debug")?
                            .call1((
                                self.template.as_ref(),
                                error.value(py),
                                self.filename
                                    .as_ref()
                                    .map(|path| path.to_string_lossy().into_owned()),
                            ))?;
                        Ok(())
                    })();
                });
            }
            error
        }
    }

    #[pymethods]
    impl Template {
        #[getter]
        fn _is_native(&self) -> bool {
            self.python_renderer.is_none()
        }

        #[getter]
        fn _has_extends(&self) -> bool {
            self.nodes
                .iter()
                .any(|node| matches!(node, TokenTree::Tag(Tag::Extends { .. })))
        }

        #[getter]
        fn _inheritance_blocks<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
            self.compiled_blocks(py)
        }

        #[getter]
        fn _django_template<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
            if let Some(renderer) = &self.python_renderer {
                renderer
                    .bind(py)
                    .call_method1("bind_origin", (self.origin(),))?;
                return Ok(renderer.bind(py).clone());
            }
            let renderer =
                self.django_renderer
                    .0
                    .get_or_try_init(py, || -> PyResult<Py<PyAny>> {
                        Ok(py
                            .import("django_rusty_templates.interop")?
                            .getattr("compile_template")?
                            .call1((
                                self.template.as_ref(),
                                PyEngine {
                                    engine: self.engine.clone(),
                                },
                                self.filename
                                    .as_ref()
                                    .map(|path| path.to_string_lossy().into_owned()),
                                self.template_name.as_deref(),
                            ))?
                            .unbind())
                    })?;
            renderer
                .bind(py)
                .call_method1("bind_origin", (self.origin(),))?;
            Ok(renderer.bind(py).clone())
        }

        #[pyo3(name = "_render")]
        fn render_inheritance(
            &self,
            py: Python<'_>,
            context: Bound<'_, PyAny>,
        ) -> PyResult<String> {
            if let Some(renderer) = &self.python_renderer {
                renderer
                    .bind(py)
                    .call_method1("bind_origin", (self.origin(),))?;
                renderer
                    .bind(py)
                    .call_method1("_render", (context,))?
                    .extract()
            } else {
                if self.has_inheritance() {
                    if !context
                        .getattr("render_context")?
                        .call_method1("get", ("block_context",))?
                        .is_none()
                    {
                        // Render the compiled native body against Django's
                        // block stack without recompiling the ancestor.
                        return self.render_django_context(py, context, true);
                    }
                    return self.py_render(py, Some(context), None);
                }
                self.render_django_context(py, context, true)
            }
        }

        fn get_exception_info<'py>(
            &self,
            py: Python<'py>,
            exception: Bound<'py, PyAny>,
            token: Bound<'py, PyAny>,
        ) -> PyResult<Bound<'py, PyAny>> {
            py.import("django_rusty_templates.diagnostics")?
                .getattr("get_exception_info")?
                .call1((self.template.as_ref(), self.origin(), exception, token))
        }

        #[getter]
        fn extra_data<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
            match &self.python_renderer {
                Some(renderer) => renderer.bind(py).getattr("extra_data"),
                None => self
                    .partial_data
                    .0
                    .get_or_try_init(py, || {
                        let data = PyDict::new(py);
                        if !self.native_partials().is_empty() {
                            let partials = PyDict::new(py);
                            let mut definitions: Vec<_> = self.native_partials().values().collect();
                            definitions
                                .sort_by_key(|definition| definition.span.0 + definition.span.1);
                            for definition in definitions {
                                let name = &definition.name;
                                let body = self.block_template(definition.nodes.clone());
                                let debug = self.engine.debug.load(Ordering::Relaxed);
                                let start = debug
                                    .then(|| self.template[..definition.span.0].chars().count());
                                let end = debug.then(|| {
                                    self.template[..definition.span.0 + definition.span.1]
                                        .chars()
                                        .count()
                                });
                                let partial = py
                                    .import("django_rusty_templates.inheritance")?
                                    .getattr("make_partial")?
                                    .call1((name, Py::new(py, body)?, self.origin(), start, end))?;
                                partials.set_item(name, partial)?;
                            }
                            data.set_item("partials", partials)?;
                        }
                        Ok(data.into_any().unbind())
                    })
                    .map(|data| data.bind(py).clone()),
            }
        }

        #[getter]
        fn name(&self) -> Option<&str> {
            self.template_name.as_deref()
        }

        #[getter]
        fn source(&self) -> &str {
            &self.template
        }

        #[getter]
        pub(crate) fn origin(&self) -> Origin {
            Origin {
                name: self.filename.as_ref().map_or_else(
                    || "<unknown source>".to_string(),
                    |filename| filename.to_string_lossy().to_string(),
                ),
                template_name: self.template_name.clone(),
                loader: if self.loader_path.is_empty() {
                    None
                } else {
                    Some(LoaderView {
                        engine: self.engine.clone(),
                        path: self.loader_path.clone(),
                    })
                },
            }
        }

        #[getter]
        fn engine(&self) -> PyEngine {
            PyEngine {
                engine: self.engine.clone(),
            }
        }

        #[getter]
        fn nodelist<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
            if let Some(renderer) = &self.python_renderer {
                return renderer.bind(py).getattr("nodelist");
            }
            // This Python tree is for Django tooling and inspection only;
            // rendering always uses the compiled Rust nodes above.
            self.inspection_nodes
                .0
                .get_or_try_init(py, || {
                    let libraries = PyDict::new(py);
                    for (name, library) in &self.engine.libraries {
                        libraries.set_item(name, library)?;
                    }
                    let builtins = PyList::empty(py);
                    for library in &self.engine.template_builtins {
                        builtins.append(library.bind(py))?;
                    }
                    Ok(py
                        .import("django_rusty_templates.inspection")?
                        .getattr("build_nodelist")?
                        .call1((
                            self.template.as_ref(),
                            libraries,
                            builtins,
                            self.origin(),
                            self.engine.debug.load(Ordering::Relaxed),
                        ))?
                        .unbind())
                })
                .map(|nodes| nodes.bind(py).clone())
        }

        fn __hash__(&self) -> u64 {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            self.template.hash(&mut hasher);
            self.filename.hash(&mut hasher);
            Arc::as_ptr(&self.engine).hash(&mut hasher);
            hasher.finish()
        }

        fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
            other.extract::<PyRef<'_, Self>>().is_ok_and(|other| {
                self.template == other.template
                    && self.filename == other.filename
                    && Arc::ptr_eq(&self.engine, &other.engine)
            })
        }

        /// Render a variable fragment inside an already-bound Django template.
        /// Read only referenced roots, keeping formatting on the live context.
        fn _render_fragment(&self, py: Python<'_>, context: Bound<'_, PyAny>) -> PyResult<String> {
            self.render_django_context(py, context, false)
        }

        #[pyo3(name = "render", signature = (context=None, request=None))]
        fn render_safe<'py>(
            &self,
            py: Python<'py>,
            context: Option<Bound<'py, PyAny>>,
            request: Option<Bound<'py, PyAny>>,
        ) -> PyResult<Bound<'py, PyAny>> {
            let rendered = self.py_render(py, context, request)?;
            MARK_SAFE
                .import(py, "django.utils.safestring", "mark_safe")?
                .call1((rendered,))
        }
    }

    impl Template {
        pub fn py_render(
            &self,
            py: Python<'_>,
            context: Option<Bound<'_, PyAny>>,
            request: Option<Bound<'_, PyAny>>,
        ) -> PyResult<String> {
            if let Some(renderer) = &self.python_renderer {
                renderer
                    .bind(py)
                    .call_method1("bind_origin", (self.origin(),))?;
                return renderer
                    .bind(py)
                    .call_method1("render", (context, request))?
                    .extract();
            }
            let mut base_context = HashMap::from([
                ("None".to_string(), py.None()),
                ("True".to_string(), PyBool::new(py, true).to_owned().into()),
                (
                    "False".to_string(),
                    PyBool::new(py, false).to_owned().into(),
                ),
            ]);
            if let Some(ref py_context) = context
                && py_context.is_instance_of::<PyContext>()
            {
                let extracted: PyContext = py_context.extract()?;
                let mut context = extracted
                    .context
                    .lock_py_attached(py)
                    .expect("Mutex should not be poisoned");
                return self._render(py, &mut context);
            }
            let context_class = DJANGO_CONTEXT.import(py, "django.template.context", "Context")?;
            let is_django_context = context
                .as_ref()
                .map(|context| context.is_instance(context_class))
                .transpose()?
                .unwrap_or(false);
            let request = match request {
                Some(request) => Some(request),
                None if is_django_context => context
                    .as_ref()
                    .and_then(|context| context.getattr("request").ok()),
                None => None,
            };
            let processor_index = if is_django_context {
                context
                    .as_ref()
                    .and_then(|context| context.getattr("_processors_index").ok())
                    .map(|index| index.extract::<usize>())
                    .transpose()?
            } else {
                None
            };
            let already_bound = is_django_context
                && context.as_ref().is_some_and(|context| {
                    context
                        .getattr("template")
                        .is_ok_and(|template| !template.is_none())
                });
            // Importing large caller contexts costs more than binding a live
            // Context for flat templates. Keep small contexts and subclasses on
            // their existing path, and scoped tags on the native scope bridge.
            // Dense templates benefit from native lookup per variable; reserve
            // this path for at least eight bindings per rendering node.
            let flat_live_context = !already_bound
                && processor_index.is_none()
                && request.is_none()
                && self
                    .nodes
                    .iter()
                    .any(|node| matches!(node, TokenTree::Variable(_) | TokenTree::Filter(_)))
                && context.as_ref().is_some_and(|input| {
                    input.get_type().as_any().is(context_class)
                        && input
                            .getattr("dicts")
                            .and_then(|dicts| dicts.cast_into::<PyList>().map_err(Into::into))
                            .is_ok_and(|dicts| {
                                dicts.iter().any(|dict| {
                                    dict.cast::<PyDict>().is_ok_and(|dict| {
                                        dict.len() >= 64 && dict.len() / 8 >= self.nodes.len()
                                    })
                                })
                            })
                })
                && self
                    .nodes
                    .iter()
                    .all(|node| !matches!(node, TokenTree::Tag(_)));
            // Opaque callbacks and native nodes can both read a caller-owned
            // Context directly. No flatten/import/writeback is needed here.
            if is_django_context
                && (self.has_python_callbacks || flat_live_context)
                && processor_index.is_none()
                && request.is_none()
            {
                let input = context.as_ref().expect("Django context checked");
                let scope = py
                    .import("django_rusty_templates.compiler")?
                    .getattr("begin_render")?
                    .call1((input, Py::new(py, self.clone())?))?;
                let result = self.render_django_context(py, input.clone(), true);
                let cleanup = scope.call_method0("close");
                if result.is_ok() {
                    cleanup?;
                }
                return result;
            }
            // Django-owned include loops already have a live, bound context.
            // Text and variable templates need no native assignment scopes, so
            // resolve their referenced roots directly rather than flattening
            // and importing every caller variable for each included child.
            if already_bound
                && self
                    .nodes
                    .iter()
                    .all(|node| !matches!(node, TokenTree::Tag(_)))
            {
                return self.render_django_context(
                    py,
                    context.expect("already_bound requires a Django context"),
                    true,
                );
            }
            let mut processor_values = HashMap::new();
            if let Some(ref request) = request
                && !already_bound
                && (!is_django_context || processor_index.is_some())
            {
                let mut processors: Vec<_> = self
                    .engine
                    .loaded_context_processors
                    .iter()
                    .map(|processor| processor.bind(py).clone())
                    .collect();
                if is_django_context
                    && let Some(ref context) = context
                    && let Ok(extra) = context.getattr("_processors")
                {
                    processors.extend(extra.try_iter()?.collect::<PyResult<Vec<_>>>()?);
                }
                for processor in processors {
                    let result = processor.call1((request,))?;
                    let values: HashMap<String, Py<PyAny>> = result.extract().map_err(|_| {
                        let name = processor
                            .getattr("__qualname__")
                            .map_or_else(|_| "unknown".to_string(), |name| name.to_string());
                        let name = if is_django_context {
                            name
                        } else {
                            let module = processor.getattr("__module__").map_or_else(
                                |_| "unknown".to_string(),
                                |module| module.to_string(),
                            );
                            format!("{module}.{name}")
                        };
                        let message = if is_django_context {
                            format!("Context processor {name} didn't return a dictionary.")
                        } else {
                            format!("Context processor '{name}' didn't return a dictionary.")
                        };
                        PyTypeError::new_err(message)
                    })?;
                    processor_values.extend(values);
                }
            }
            let mut autoescape = self.engine.autoescape;
            let mut use_l10n = None;
            let mut use_tz = None;
            if let Some(ref context) = context {
                if is_django_context {
                    autoescape = context.getattr("autoescape")?.extract()?;
                    use_l10n = context.getattr("use_l10n")?.extract()?;
                    use_tz = context.getattr("use_tz")?.extract()?;
                    if let Some(index) = processor_index.filter(|_| !already_bound) {
                        for (position, dict) in context.getattr("dicts")?.try_iter()?.enumerate() {
                            if position == index {
                                base_context.extend(processor_values.drain());
                            } else {
                                base_context.extend(dict?.extract::<HashMap<String, Py<PyAny>>>()?);
                            }
                        }
                    } else {
                        base_context.extend(context.call_method0("flatten")?.extract::<HashMap<
                            String,
                            Py<PyAny>,
                        >>(
                        )?);
                    }
                } else {
                    base_context.extend(processor_values);
                    base_context.extend(context.extract::<HashMap<String, Py<PyAny>>>()?);
                }
            } else {
                base_context.extend(processor_values);
            }
            let mut rendering_context =
                Context::new(base_context, request.map(pyo3::Bound::unbind), autoescape);
            rendering_context.use_l10n = use_l10n;
            rendering_context.use_tz = use_tz;
            rendering_context.format_values = true;
            if is_django_context && let Some(input) = &context {
                if !already_bound {
                    input.setattr("template_name", self.template_name.as_deref())?;
                }
                rendering_context.callback_input = Some(Arc::new(
                    py.import("weakref")?
                        .getattr("ref")?
                        .call1((input,))?
                        .unbind(),
                ));
            }
            let result = self._render(py, &mut rendering_context);
            if is_django_context && let Some(input) = &context {
                input.setattr("autoescape", rendering_context.autoescape)?;
                input.setattr("use_l10n", rendering_context.use_l10n)?;
                input.setattr("use_tz", rendering_context.use_tz)?;
            }
            if let Some(context) = context {
                rendering_context.write_assignments(py, &context)?;
            }
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use pyo3::types::PyAnyMethods;
    use std::sync::Arc;

    use super::django_rusty_templates::*;

    use pyo3::Python;
    use pyo3::types::{PyDict, PyDictMethods, PyList, PyString};

    #[test]
    fn test_syntax_error() {
        Python::initialize();

        Python::attach(|py| {
            let mut filename = std::env::current_dir().unwrap();
            filename.push("tests");
            filename.push("templates");
            filename.push("parse_error.txt");

            let expected = format!(
                "TemplateSyntaxError:   × Empty variable tag
   ╭─[{}:1:28]
 1 │ This is an empty variable: {{{{ }}}}
   ·                            ──┬──
   ·                              ╰── here
   ╰────
",
                filename.display(),
            );

            let engine = Arc::new(Engine::empty());
            let template_string = std::fs::read_to_string(&filename).unwrap();
            let error = temp_env::with_var("NO_COLOR", Some("1"), || {
                Template::new(py, &template_string, filename, "parse_error.txt", engine)
                    .unwrap_err()
            });

            let error_string = format!(
                "TemplateSyntaxError: {}",
                error.value(py).getattr("rusty_diagnostic").unwrap()
            );
            assert_eq!(error_string, expected);
        });
    }

    #[test]
    fn test_syntax_error_from_string() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Arc::new(Engine::empty());
            let template_string = "{{ foo.bar|title'foo' }}".to_string();
            let error = temp_env::with_var("NO_COLOR", Some("1"), || {
                Template::new_from_string(py, template_string, engine).unwrap_err()
            });

            let expected = "TemplateSyntaxError:   × Could not parse the remainder
   ╭────
 1 │ {{ foo.bar|title'foo' }}
   ·                 ──┬──
   ·                   ╰── here
   ╰────
";

            let error_string = format!(
                "TemplateSyntaxError: {}",
                error.value(py).getattr("rusty_diagnostic").unwrap()
            );
            assert_eq!(error_string, expected);
        });
    }

    #[test]
    fn test_render_empty_template() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Arc::new(Engine::empty());
            let template_string = String::new();
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let context = PyDict::new(py).into_any();

            assert_eq!(template.py_render(py, Some(context), None).unwrap(), "");
        });
    }

    #[test]
    fn test_render_template_variable() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Arc::new(Engine::empty());
            let template_string = "Hello {{ user }}!".to_string();
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let context = PyDict::new(py);
            context.set_item("user", "Lily").unwrap();

            assert_eq!(
                template
                    .py_render(py, Some(context.into_any()), None)
                    .unwrap(),
                "Hello Lily!"
            );
        });
    }

    #[test]
    fn test_render_template_unknown_variable() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Arc::new(Engine::empty());
            let template_string = "Hello {{ user }}!".to_string();
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let context = PyDict::new(py).into_any();

            assert_eq!(
                template.py_render(py, Some(context), None).unwrap(),
                "Hello !"
            );
        });
    }

    #[test]
    fn test_render_template_variable_nested() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Arc::new(Engine::empty());
            let template_string = "Hello {{ user.profile.names.0 }}!".to_string();
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let locals = PyDict::new(py);
            py.run(
                cr#"
class User:
    def __init__(self, names):
        self.profile = {"names": names}

user = User(["Lily"])
"#,
                None,
                Some(&locals),
            )
            .unwrap();
            let user = locals.get_item("user").unwrap().unwrap();
            let context = PyDict::new(py);
            context.set_item("user", user.into_any()).unwrap();

            assert_eq!(
                template
                    .py_render(py, Some(context.into_any()), None)
                    .unwrap(),
                "Hello Lily!"
            );
        });
    }

    #[test]
    fn test_engine_from_string() {
        Python::initialize();

        Python::attach(|py| {
            let engine = PyEngine::new(
                py,
                None,
                false,
                None,
                false,
                None,
                String::new(),
                "utf-8".to_string(),
                None,
                None,
                false,
            )
            .unwrap();
            let template_string = PyString::new(py, "Hello {{ user }}!");
            let template = engine.from_string(template_string).unwrap();
            let context = PyDict::new(py).into_any();

            assert_eq!(
                template.py_render(py, Some(context), None).unwrap(),
                "Hello !"
            );
        });
    }

    #[test]
    fn test_clone_template() {
        use std::collections::HashMap;

        use pyo3::IntoPyObject;
        use pyo3::types::{PyAnyMethods, PyListMethods};

        Python::initialize();

        Python::attach(|py| {
            let cwd = std::env::current_dir().unwrap();
            let sys_path = py.import("sys").unwrap().getattr("path").unwrap();
            let sys_path = sys_path.cast().unwrap();
            sys_path.append(cwd.to_string_lossy()).unwrap();
            let engine = PyEngine::new(
                py,
                Some(vec!["tests/templates"].into_pyobject(py).unwrap()),
                false,
                None,
                false,
                None,
                String::new(),
                "utf-8".to_string(),
                Some(
                    HashMap::from([("custom_filters", "tests.templatetags.custom_filters")])
                        .into_pyobject(py)
                        .unwrap()
                        .into_any(),
                ),
                None,
                false,
            )
            .unwrap();
            let template = engine
                .get_template_native(py, Some(PyString::new(py, "full_example.html").into_any()))
                .unwrap();
            let cloned = template.clone();
            assert_eq!(cloned.filename, template.filename);
            assert_eq!(cloned.template, template.template);
            assert_eq!(cloned.nodes, template.nodes);
            assert_eq!(cloned.engine.autoescape, template.engine.autoescape);
        });
    }

    #[test]
    fn test_engine_attributes() {
        use std::collections::HashMap;

        use pyo3::IntoPyObject;
        use pyo3::types::{PyAnyMethods, PyListMethods};

        Python::initialize();

        Python::attach(|py| {
            let cwd = std::env::current_dir().unwrap();
            let sys_path = py.import("sys").unwrap().getattr("path").unwrap();
            let sys_path = sys_path.cast().unwrap();
            sys_path.append(cwd.to_string_lossy()).unwrap();

            let engine = PyEngine::new(
                py,
                Some(
                    vec!["tests/templates", "other/templates"]
                        .into_pyobject(py)
                        .unwrap(),
                ),
                true,
                Some(
                    vec!["django.template.context_processors.debug"]
                        .into_pyobject(py)
                        .unwrap(),
                ),
                true,
                None,
                "INVALID".to_string(),
                "utf-8".to_string(),
                Some(
                    HashMap::from([("custom_filters", "tests.templatetags.custom_filters")])
                        .into_pyobject(py)
                        .unwrap()
                        .into_any(),
                ),
                None,
                false,
            )
            .unwrap();

            let py_engine = engine.into_pyobject(py).unwrap();
            py_engine.getattr("dirs").unwrap();
            py_engine.getattr("app_dirs").unwrap();
            py_engine.getattr("context_processors").unwrap();
            py_engine.getattr("debug").unwrap();
            py_engine.getattr("string_if_invalid").unwrap();
            py_engine.getattr("file_charset").unwrap();
            py_engine.getattr("builtins").unwrap();
            py_engine.getattr("libraries").unwrap();
            py_engine.getattr("autoescape").unwrap();

            // Non-trivial getters
            let dirs: Vec<String> = py_engine.getattr("dirs").unwrap().extract().unwrap();
            assert_eq!(dirs.len(), 2);
            assert!(dirs[0].ends_with("tests/templates"));
            assert!(dirs[1].ends_with("other/templates"));

            let file_charset: String = py_engine
                .getattr("file_charset")
                .unwrap()
                .extract()
                .unwrap();
            assert_eq!(file_charset, "utf-8");

            // TODO: support this once #89 lands
            // let loaders: Vec<String> = py_engine.getattr("loaders").unwrap().extract().unwrap();
            // assert_eq!(loaders.len(), 1);
            // assert_eq!(loaders[0], "django.template.loaders.cached.Loader");
        });
    }

    #[test]
    fn test_engine_app_dirs_with_loaders() {
        Python::initialize();

        Python::attach(|py| {
            let loaders = PyList::empty(py).into_any();
            let app_dirs = true;
            let engine_error = PyEngine::new(
                py,
                None,
                app_dirs,
                None,
                false,
                Some(loaders),
                String::new(),
                "utf-8".to_string(),
                None,
                None,
                false,
            )
            .unwrap_err();

            assert!(engine_error.is_instance_of::<ImproperlyConfigured>(py));
            let message = "app_dirs must not be set when loaders is defined.";
            assert_eq!(engine_error.value(py).to_string(), message);
        });
    }
}
