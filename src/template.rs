use pyo3::prelude::*;

#[pymodule]
pub mod django_rusty_templates {
    use std::borrow::Cow;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use encoding_rs::Encoding;
    use pyo3::exceptions::{
        PyAttributeError, PyImportError, PyNotImplementedError, PyOverflowError, PyTypeError,
        PyValueError,
    };
    use pyo3::import_exception;
    use pyo3::intern;
    use pyo3::prelude::*;
    use pyo3::sync::{MutexExt, PyOnceLock};
    use pyo3::types::{PyBool, PyDict, PyIterator, PyList, PyString, PyTuple};

    use crate::error::{PyRenderError, RenderError};
    use crate::loaders::{AppDirsLoader, CachedLoader, FileSystemLoader, Loader, LocMemLoader};
    use crate::parse::{Parser, TokenTree};
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

    static IMPORT_STRING: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

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

        fn _cache_template(
            &self,
            py: Python<'_>,
            key: String,
            renderer: Bound<'_, PyAny>,
        ) -> PyResult<()> {
            let origin = renderer.getattr("origin")?;
            let loader = origin
                .getattr("loader")?
                .extract::<PyRef<'_, LoaderView>>()?;
            let template = Template {
                filename: Some(PathBuf::from(origin.getattr("name")?.extract::<String>()?)),
                template: renderer.getattr("source")?.extract()?,
                nodes: Vec::new(),
                engine: self.engine.clone(),
                template_name: Some(renderer.getattr("name")?.extract()?),
                loader_path: loader.path[self.path.len()..].to_vec(),
                inspection_nodes: Arc::new(InspectionNodes(PyOnceLock::new())),
                python_renderer: Some(Arc::new(renderer.unbind())),
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
    struct Origin {
        #[pyo3(get)]
        name: String,
        #[pyo3(get)]
        template_name: Option<String>,
        #[pyo3(get)]
        loader: Option<LoaderView>,
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
            other
                .extract::<PyRef<'_, Self>>()
                .is_ok_and(|other| self.name == other.name && self.loader == other.loader)
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
        loaded_context_processors: Vec<Py<PyAny>>,
        pub libraries: HashMap<String, Py<PyAny>>,
    }

    impl Engine {
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
    #[pyclass(name = "Engine")]
    pub struct PyEngine {
        engine: Arc<Engine>,
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
            let engine = Engine {
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
        pub fn get_template(
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
            &self,
            py: Python<'py>,
            name: Bound<'py, PyAny>,
            dirs: Option<Bound<'py, PyAny>>,
            skip: Option<Bound<'py, PyAny>>,
        ) -> PyResult<Bound<'py, PyAny>> {
            let _ = dirs;
            py.import("django_rusty_templates.interop")?
                .getattr("find_template")?
                .call1((
                    Self {
                        engine: self.engine.clone(),
                    },
                    name,
                    skip,
                ))
        }

        /// Given a list of template names, return the first that can be loaded.
        ///
        /// See <https://docs.djangoproject.com/en/stable/ref/templates/api/#django.template.Engine.select_template>
        pub fn select_template(
            &mut self,
            py: Python<'_>,
            template_name_list: Vec<String>,
        ) -> PyResult<Template> {
            select_template(self.engine.clone(), py, template_name_list)
        }

        #[allow(clippy::wrong_self_convention)] // We're implementing a Django interface
        pub fn from_string(&self, template_code: Bound<'_, PyString>) -> PyResult<Template> {
            Template::new_from_string(
                template_code.py(),
                template_code.extract()?,
                self.engine.clone(),
            )
        }

        /// Render the template specified by `template_name` with the given context.
        /// For use in Django's test suite.
        #[pyo3(signature = (template_name, context=None))]
        pub fn render_to_string(
            &mut self,
            py: Python<'_>,
            template_name: Bound<'_, PyAny>,
            context: Option<Bound<'_, PyAny>>,
        ) -> PyResult<String> {
            let template = if template_name.is_instance_of::<PyList>()
                || template_name.is_instance_of::<PyTuple>()
            {
                self.select_template(py, template_name.extract()?)?
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
        fn template_loaders<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
            let count = self
                .engine
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
                    engine: self.engine.clone(),
                    path: vec![index],
                },))?)?;
            }
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
        pub template: String,
        pub nodes: Vec<TokenTree>,
        pub engine: Arc<Engine>,
        pub template_name: Option<String>,
        pub loader_path: Vec<usize>,
        inspection_nodes: Arc<InspectionNodes>,
        python_renderer: Option<Arc<Py<PyAny>>>,
    }

    fn canonical_parse_error(
        py: Python<'_>,
        error: crate::parse::PyParseError,
        engine: Arc<Engine>,
        source: &str,
        filename: Option<&Path>,
        template_name: Option<&str>,
    ) -> PyErr {
        let native = match error.try_into_parse_error() {
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
        };
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
                let diagnostic = native.value(py).to_string();
                let _ = error.value(py).setattr("rusty_diagnostic", &diagnostic);
                let _ = error.value(py).call_method1("add_note", (diagnostic,));
                error
            }
            Ok(()) => native,
        }
    }

    impl Template {
        pub fn new(
            py: Python<'_>,
            template: &str,
            filename: PathBuf,
            template_name: &str,
            engine: Arc<Engine>,
        ) -> PyResult<Self> {
            let mut parser = Parser::new(
                py,
                TemplateString(template),
                engine.clone(),
                Some(template_name),
            );
            let mut python_renderer = None;
            let parsed = if template.contains("..")
                && py
                    .import("django")?
                    .getattr("VERSION")?
                    .get_item(0)?
                    .extract::<u8>()?
                    >= 6
                && py
                    .import("django")?
                    .getattr("VERSION")?
                    .get_item(1)?
                    .extract::<u8>()?
                    >= 1
            {
                Err(crate::parse::PyParseError::PyErr(
                    PyNotImplementedError::new_err(
                        "not yet implemented: Django 6.1 variable lookup",
                    ),
                ))
            } else {
                parser.parse()
            };
            let nodes = match parsed {
                Ok(nodes) => nodes,
                Err(crate::parse::PyParseError::PyErr(error))
                    if error.is_instance_of::<PyNotImplementedError>(py)
                        && [
                            "block",
                            "extends",
                            "Fully custom tag",
                            "Django 6.1 variable lookup",
                        ]
                        .iter()
                        .any(|tag| {
                            error.value(py).to_string() == format!("not yet implemented: {tag}")
                        }) =>
                {
                    python_renderer = Some(Arc::new(
                        py.import("django_rusty_templates.interop")?
                            .getattr("compile_template")?
                            .call1((
                                template,
                                PyEngine {
                                    engine: engine.clone(),
                                },
                                Some(&filename).map(|path| path.to_string_lossy().into_owned()),
                                Some(template_name),
                            ))?
                            .unbind(),
                    ));
                    Vec::new()
                }
                Err(error) => {
                    return Err(canonical_parse_error(
                        py,
                        error,
                        engine.clone(),
                        template,
                        Some(&filename),
                        Some(template_name),
                    ));
                }
            };
            Ok(Self {
                template: template.to_string(),
                filename: Some(filename),
                template_name: Some(template_name.to_string()),
                loader_path: Vec::new(),
                inspection_nodes: Arc::new(InspectionNodes(PyOnceLock::new())),
                python_renderer,
                nodes,
                engine,
            })
        }

        pub fn new_from_string(
            py: Python<'_>,
            template: String,
            engine: Arc<Engine>,
        ) -> PyResult<Self> {
            let mut parser = Parser::new(py, TemplateString(&template), engine.clone(), None);
            let mut python_renderer = None;
            let parsed = if template.contains("..")
                && py
                    .import("django")?
                    .getattr("VERSION")?
                    .get_item(0)?
                    .extract::<u8>()?
                    >= 6
                && py
                    .import("django")?
                    .getattr("VERSION")?
                    .get_item(1)?
                    .extract::<u8>()?
                    >= 1
            {
                Err(crate::parse::PyParseError::PyErr(
                    PyNotImplementedError::new_err(
                        "not yet implemented: Django 6.1 variable lookup",
                    ),
                ))
            } else {
                parser.parse()
            };
            let nodes = match parsed {
                Ok(nodes) => nodes,
                Err(crate::parse::PyParseError::PyErr(error))
                    if error.is_instance_of::<PyNotImplementedError>(py)
                        && [
                            "block",
                            "extends",
                            "Fully custom tag",
                            "Django 6.1 variable lookup",
                        ]
                        .iter()
                        .any(|tag| {
                            error.value(py).to_string() == format!("not yet implemented: {tag}")
                        }) =>
                {
                    python_renderer = Some(Arc::new(
                        py.import("django_rusty_templates.interop")?
                            .getattr("compile_template")?
                            .call1((
                                &template,
                                PyEngine {
                                    engine: engine.clone(),
                                },
                                None::<String>,
                                None::<String>,
                            ))?
                            .unbind(),
                    ));
                    Vec::new()
                }
                Err(error) => {
                    return Err(canonical_parse_error(
                        py,
                        error,
                        engine.clone(),
                        &template,
                        None,
                        None,
                    ));
                }
            };
            Ok(Self {
                template,
                filename: None,
                template_name: None,
                loader_path: Vec::new(),
                inspection_nodes: Arc::new(InspectionNodes(PyOnceLock::new())),
                python_renderer,
                nodes,
                engine,
            })
        }

        pub fn render(&self, py: Python<'_>, context: &mut Context) -> RenderResult<'_> {
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
            let previous_invalid = std::mem::replace(
                &mut context.string_if_invalid,
                self.engine.string_if_invalid.clone(),
            );
            let result = (|| {
                let mut rendered = String::with_capacity(self.template.len());
                let template = TemplateString(&self.template);
                for node in &self.nodes {
                    match node.render(py, template, context) {
                        Err(PyRenderError::PyErr(error)) => {
                            if self.engine.debug.load(Ordering::Relaxed) {
                                let _ = (|| -> PyResult<()> {
                                    py.import("django_rusty_templates.diagnostics")?
                                        .getattr("attach_debug")?
                                        .call1((
                                            self.template.as_str(),
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
                        result => rendered.push_str(&result?),
                    }
                }
                Ok(Cow::Owned(rendered))
            })();
            context.string_if_invalid = previous_invalid;
            result
        }

        fn _render(&self, py: Python<'_>, context: &mut Context) -> PyResult<String> {
            match self.render(py, context) {
                Ok(content) => Ok(content.to_string()),
                Err(err) => {
                    let err = err.try_into_render_error()?;
                    match err {
                        RenderError::RelativePathError(_) => {
                            Err(TemplateSyntaxError::with_source_code(
                                err.into(),
                                self.template.clone(),
                            ))
                        }
                        RenderError::VariableDoesNotExist { .. }
                        | RenderError::ArgumentDoesNotExist { .. } => {
                            Err(VariableDoesNotExist::with_source_code(
                                err.into(),
                                self.template.clone(),
                            ))
                        }
                        RenderError::InvalidArgumentInteger { .. }
                        | RenderError::InvalidArgumentString { .. }
                        | RenderError::TupleUnpackError { .. } => Err(
                            PyValueError::with_source_code(err.into(), self.template.clone()),
                        ),
                        RenderError::OverflowError { .. }
                        | RenderError::InvalidArgumentFloat { .. } => Err(
                            PyOverflowError::with_source_code(err.into(), self.template.clone()),
                        ),
                    }
                }
            }
        }
    }

    #[pymethods]
    impl Template {
        #[getter]
        fn _django_template<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
            if let Some(renderer) = &self.python_renderer {
                renderer
                    .bind(py)
                    .call_method1("bind_origin", (self.origin(),))?;
                return Ok(renderer.bind(py).clone());
            }
            py.import("django_rusty_templates.interop")?
                .getattr("compile_template")?
                .call1((
                    self.template.as_str(),
                    PyEngine {
                        engine: self.engine.clone(),
                    },
                    self.filename
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned()),
                    self.template_name.as_deref(),
                ))
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
                self.py_render(py, Some(context), None)
            }
        }

        #[getter]
        fn extra_data<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
            match &self.python_renderer {
                Some(renderer) => renderer.bind(py).getattr("extra_data"),
                None => Ok(PyDict::new(py).into_any()),
            }
        }

        #[getter]
        fn source(&self) -> &str {
            &self.template
        }

        #[getter]
        fn origin(&self) -> Origin {
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
                    let base = py.import("django.template.base")?;
                    let tokens = base
                        .getattr("Lexer")?
                        .call1((&self.template,))?
                        .call_method0("tokenize")?;
                    let libraries = PyDict::new(py);
                    for (name, library) in &self.engine.libraries {
                        libraries.set_item(name, library)?;
                    }
                    let builtins = PyList::empty(py);
                    for library in &self.engine.template_builtins {
                        builtins.append(library.bind(py))?;
                    }
                    Ok(base
                        .getattr("Parser")?
                        .call1((tokens, libraries, builtins, self.origin()))?
                        .call_method0("parse")?
                        .unbind())
                })
                .map(|nodes| nodes.bind(py).clone())
        }

        fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
            other.extract::<PyRef<'_, Self>>().is_ok_and(|other| {
                self.template == other.template
                    && self.filename == other.filename
                    && Arc::ptr_eq(&self.engine, &other.engine)
            })
        }

        #[pyo3(name = "render", signature = (context=None, request=None))]
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
            let result = self._render(py, &mut rendering_context);
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
                .get_template(py, Some(PyString::new(py, "full_example.html").into_any()))
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
