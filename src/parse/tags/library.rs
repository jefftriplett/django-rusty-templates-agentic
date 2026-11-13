//! `{% load %}` and tags registered by Python template libraries.
use crate::gc::Gc;
use crate::parse::EndTagType;
use crate::parse::Parse;
use crate::parse::ParseError;
use crate::parse::Parser;
use crate::parse::PyParseError;
use crate::parse::PythonNode;
use crate::parse::SimpleBlockTag;
use crate::parse::SimpleTag;
use crate::parse::Tag;
use crate::parse::TagElement;
use crate::parse::TokenTree;
use crate::parse::Unimplemented;
use crate::parse::extract_as_variable;
use crate::parse::stock;
use dtl_lexer::core::Lexer;
use dtl_lexer::core::TokenType;
use dtl_lexer::tag::TagParts;
use dtl_lexer::tag::kwarg::TagElementKwargLexer;
use dtl_lexer::tag::load::LoadLexer;
use dtl_lexer::tag::load::LoadToken;
use dtl_lexer::types::At;
use dtl_lexer::types::TemplateString;
use pyo3::intern;
use pyo3::prelude::*;
use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::HashSet;

trait LoadLibrary {
    fn load_library<'l, 'py>(
        &self,
        py: Python<'py>,
        libraries: &'l HashMap<String, Py<PyAny>>,
        template: TemplateString<'_>,
    ) -> Result<&'l Bound<'py, PyAny>, ParseError>;
}
impl LoadLibrary for LoadToken {
    fn load_library<'l, 'py>(
        &self,
        py: Python<'py>,
        libraries: &'l HashMap<String, Py<PyAny>>,
        template: TemplateString<'_>,
    ) -> Result<&'l Bound<'py, PyAny>, ParseError> {
        let library_name = template.content(self.at);
        match libraries.get(library_name) {
            Some(library) => Ok(library.bind(py)),
            None => {
                let mut libraries: Vec<_> = libraries.keys().map(String::as_str).collect();
                libraries.sort_unstable();
                let help = format!("Must be one of:\n{}", libraries.join("\n"));
                Err(ParseError::MissingTagLibrary {
                    at: self.at.into(),
                    library: library_name.to_string(),
                    help,
                })
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(in crate::parse) struct SimpleTagContext<'py> {
    func: Bound<'py, PyAny>,
    function_name: String,
    takes_context: bool,
    params: Vec<String>,
    defaults_count: usize,
    varargs: bool,
    kwonly: Vec<String>,
    kwonly_defaults: HashSet<String>,
    varkw: bool,
}

#[derive(Clone)]
pub(in crate::parse) enum TagContext<'py> {
    Stock(stock::StockCompiler),
    Unloaded(Bound<'py, PyAny>),
    Simple(SimpleTagContext<'py>),
    Inclusion {
        context: SimpleTagContext<'py>,
        filename: Bound<'py, PyAny>,
    },
    AdminInclusion {
        context: SimpleTagContext<'py>,
        template_name: &'static str,
    },
    SimpleBlock {
        end_tag_name: String,
        context: SimpleTagContext<'py>,
    },
    EndSimpleBlock,
    Opaque(Bound<'py, PyAny>),
}

/// The free variables of the `compile_func` closure Django generates for
/// `simple_tag`, `simple_block_tag` and `inclusion_tag`.
const COMPILE_FUNC_CELLS: [&str; 9] = [
    "defaults",
    "func",
    "function_name",
    "kwonly",
    "kwonly_defaults",
    "params",
    "takes_context",
    "varargs",
    "varkw",
];

impl<'py> Parser<'_, 'py> {
    #[allow(clippy::type_complexity)]
    fn parse_custom_tag_parts(
        &self,
        parts: TagParts,
        context: &SimpleTagContext,
    ) -> Result<(Vec<TagElement>, Vec<(String, TagElement)>, Option<String>), ParseError> {
        let mut args = Vec::new();
        let mut kwargs = Vec::new();

        let parts_at = parts.at;
        let mut prev_at = parts.at;
        let mut seen_kwargs: HashMap<&str, At> = HashMap::new();
        let params_count = context.params.len();
        let mut tokens = TagElementKwargLexer::new(self.template, parts)
            .with_modern_syntax(self.engine.django_series >= (6, 0))
            .collect::<Result<Vec<_>, _>>()?;
        let asvar = extract_as_variable(&mut tokens, &self.template)?;
        for (index, token) in tokens.iter().enumerate() {
            match token.kwarg {
                None => {
                    if !seen_kwargs.is_empty() {
                        return Err(ParseError::PositionalAfterKeyword {
                            at: token.at.into(),
                            after: prev_at.into(),
                        });
                    }
                    if !context.varargs && index == params_count {
                        return Err(ParseError::TooManyPositionalArguments {
                            at: token.at.into(),
                        });
                    }
                    let element = token.parse(self)?;
                    args.push(element);
                    prev_at = token.at;
                }
                Some(name_at) => {
                    let kwarg_at = (name_at.0, name_at.1 + 1 + token.at.1);
                    let name = self.template.content(name_at);
                    if !context.varkw
                        && !context.params.iter().any(|a| a == name)
                        && !context.kwonly.iter().any(|kw| kw == name)
                    {
                        return Err(ParseError::UnexpectedKeywordArgument {
                            at: kwarg_at.into(),
                        });
                    } else if let Some(&first_at) = seen_kwargs.get(name) {
                        return Err(ParseError::DuplicateKeywordArgument {
                            tag_name: context.function_name.clone(),
                            kwarg_name: name.to_string(),
                            first_at: first_at.into(),
                            second_at: kwarg_at.into(),
                        });
                    }
                    let element = token.parse(self)?;
                    seen_kwargs.insert(name, kwarg_at);
                    kwargs.push((name.to_string(), element));
                    prev_at = kwarg_at;
                }
            }
        }

        let args_count = args.len();
        let mut missing_params = Vec::new();
        if params_count > args_count + context.defaults_count {
            for param in &context.params[args_count..params_count - context.defaults_count] {
                if !seen_kwargs.contains_key(param.as_str()) {
                    missing_params.push(param.clone());
                }
            }
        }
        for kwarg in &context.kwonly {
            if !seen_kwargs.contains_key(kwarg.as_str())
                && !context.kwonly_defaults.contains(kwarg.as_str())
            {
                missing_params.push(kwarg.clone());
            }
        }
        if !missing_params.is_empty() {
            let missing = missing_params
                .iter()
                .map(|p| format!("'{p}'"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ParseError::MissingArguments {
                tag_name: context.function_name.clone(),
                at: parts_at.into(),
                missing,
            });
        }
        Ok((args, kwargs, asvar))
    }

    pub(in crate::parse) fn parse_simple_tag(
        &self,
        context: &SimpleTagContext,
        at: At,
        parts: TagParts,
    ) -> Result<TokenTree, PyParseError> {
        let (args, kwargs, target_var) = self.parse_custom_tag_parts(parts, context)?;
        let tag = SimpleTag {
            func: context.func.clone().unbind().into(),
            at,
            takes_context: context.takes_context,
            args,
            kwargs,
            target_var,
        };
        Ok(TokenTree::tag(Tag::SimpleTag(tag)))
    }

    pub(in crate::parse) fn parse_simple_block_tag(
        &mut self,
        context: SimpleTagContext,
        tag_name: String,
        end_tag_name: String,
        at: At,
        parts: TagParts,
    ) -> Result<TokenTree, PyParseError> {
        let (args, kwargs, target_var) = self.parse_custom_tag_parts(parts, &context)?;
        let (nodes, _) = self.parse_until(
            vec![EndTagType::Custom(end_tag_name)],
            Cow::Owned(tag_name),
            at,
        )?;
        let tag = SimpleBlockTag {
            func: context.func.clone().unbind().into(),
            nodes,
            at,
            takes_context: context.takes_context,
            args,
            kwargs,
            target_var,
        };
        Ok(TokenTree::tag(Tag::SimpleBlockTag(tag)))
    }

    pub(in crate::parse) fn parse_load(
        &mut self,
        at: At,
        parts: TagParts,
    ) -> Result<TokenTree, PyParseError> {
        if let Some(parser) = &self.compiler_parser {
            parser
                .bind(self.py)
                .call_method1("load", (self.template.content(at),))?;
        }
        self.compiler_loads.push(at);
        let tokens: Vec<_> = LoadLexer::new(self.template, parts).collect();
        let mut rev = tokens.iter().rev();
        if let (Some(last), Some(prev)) = (rev.next(), rev.next())
            && tokens.len() > 2
            && self.template.content(prev.at) == "from"
        {
            let library = last.load_library(self.py, &self.engine.libraries, self.template)?;
            let filters = self.get_filters(library)?;
            let tags = self.get_tags(library)?;
            for token in rev {
                let content = self.template.content(token.at);
                if let Some(filter) = filters.get(content) {
                    self.external_filters
                        .insert(content.to_string(), filter.clone());
                } else if let Some(tag) = tags.get(content) {
                    self.register_tag(content, tag.clone())?;
                } else {
                    return Err(ParseError::MissingFilterTag {
                        library: self.template.content(last.at).to_string(),
                        library_at: last.at.into(),
                        tag: content.to_string(),
                        tag_at: token.at.into(),
                    }
                    .into());
                }
            }
            return Ok(TokenTree::tag(Tag::Load));
        }
        for token in tokens {
            let library = token.load_library(self.py, &self.engine.libraries, self.template)?;
            let filters = self.get_filters(library)?;
            let tags = self.get_tags(library)?;
            self.external_filters.extend(filters);
            for (name, tag) in tags {
                self.register_tag(&name, tag)?;
            }
        }
        Ok(TokenTree::tag(Tag::Load))
    }

    /// Whether a library tag used in this template needs Django's full
    /// parser. Every opaque tag asks the same question, so the answer is
    /// computed once, from one lex of the template.
    fn mixed_compilers(&mut self) -> bool {
        if let Some(mixed) = self.mixed_compilers {
            return mixed;
        }
        let template = self.template;
        let tag_names: HashSet<&str> = Lexer::new(template)
            .filter(|token| token.token_type == TokenType::Tag)
            .filter_map(|token| token.content(template).split_whitespace().next())
            .collect();
        let mixed = self
            .engine
            .libraries
            .values()
            .chain(self.engine.template_builtins.iter().skip(3))
            .any(|library| {
                let Ok(tags) = self.get_tags(library.bind(self.py)) else {
                    return true;
                };
                tags.iter().any(|(name, candidate)| {
                    if !tag_names.contains(name.as_str()) {
                        return false;
                    }
                    if stock::compiler(self.py, candidate).is_ok_and(|kind| kind.is_some())
                        || stock::admin_inclusion(self.py, candidate)
                            .is_ok_and(|kind| kind.is_some())
                    {
                        return false;
                    }
                    let generated = candidate
                        .getattr("__closure__")
                        .is_ok_and(|closure| !closure.is_none())
                        && candidate.getattr("__code__").is_ok_and(|code| {
                            code.getattr("co_name").is_ok_and(|name| {
                                name.extract::<String>()
                                    .is_ok_and(|name| name == "compile_func")
                            }) && code.getattr("co_freevars").is_ok_and(|names| {
                                names.extract::<Vec<String>>().is_ok_and(|names| {
                                    COMPILE_FUNC_CELLS
                                        .iter()
                                        .all(|required| names.iter().any(|name| name == required))
                                })
                            })
                        });
                    let full_parser = candidate
                        .getattr("rusty_requires_full_parser")
                        .is_ok_and(|value| value.is_truthy().unwrap_or(true));
                    !generated && (full_parser || !self.opted_in(candidate))
                })
            });
        self.mixed_compilers = Some(mixed);
        mixed
    }

    /// Whether a compiler opted into isolated compilation, with its own
    /// `rusty_isolated_compiler` attribute or the engine's `isolated_tags`.
    fn opted_in(&self, compiler: &Bound<'py, PyAny>) -> bool {
        compiler
            .getattr("rusty_isolated_compiler")
            .is_ok_and(|value| value.is_truthy().unwrap_or(false))
            || self
                .engine
                .isolated_compilers
                .iter()
                .any(|opted_in| opted_in.is(compiler))
    }

    fn register_opaque(
        &mut self,
        name: &str,
        compiler: &Bound<'py, PyAny>,
    ) -> Result<(), PyParseError> {
        if self.inspect_only
            || !self.opted_in(compiler)
            || compiler
                .getattr("rusty_requires_full_parser")
                .is_ok_and(|value| value.is_truthy().unwrap_or(true))
            || self.mixed_compilers()
        {
            if self.compiler_parser.is_some() {
                let error = crate::template::django_rusty_templates::TemplateSyntaxError::new_err(
                    "An isolated compiler registered a compiler requiring the full parser. Leave the registering compiler unannotated or opt its replacement in.",
                );
                error
                    .value(self.py)
                    .setattr("_rusty_compiler_error", true)?;
                return Err(error.into());
            }
            return Err(Unimplemented::FullyCustomTag.into());
        }
        self.external_tags
            .insert(name.to_string(), TagContext::Opaque(compiler.clone()));
        Ok(())
    }

    pub(in crate::parse) fn parse_opaque(
        &mut self,
        at: At,
        compiler: &Bound<'py, PyAny>,
    ) -> Result<TokenTree, PyParseError> {
        if self.compiler_parser.is_none() {
            let parser = self
                .py
                .import("django_rusty_templates.compiler")?
                .getattr("CompilerParser")?
                .call1((
                    self.template.0,
                    crate::template::django_rusty_templates::PyEngine {
                        engine: self.engine.clone(),
                    },
                    self.warning_origin.as_deref(),
                    self.origin,
                ))?;
            for span in &self.compiler_loads {
                parser.call_method1("load", (self.template.content(*span),))?;
            }
            // Ordinary native templates never need a second tag registry.
            // Snapshot only once, before an opaque compiler can replace tags.
            self.registered_compilers = Some(self.get_tags(&parser)?);
            self.compiler_parser = Some(parser.unbind());
        }
        let parser = self
            .compiler_parser
            .as_ref()
            .expect("compiler parser initialized")
            .bind(self.py);
        let (node, end): (Py<PyAny>, usize) = parser
            .call_method1("compile", (at.0, compiler))?
            .extract()?;
        if self.shares_parser_state(at.0 + at.1, end) {
            // Django's parser compiled these tags inside the opaque body, out
            // of reach of the native inheritance, cycle and partial
            // registries. Compile the whole template with Django instead,
            // which runs this compiler a second time.
            return Err(Unimplemented::FullyCustomTag.into());
        }
        let filters = self.get_filters(parser)?;
        let tags = self.get_tags(parser)?;
        self.external_filters.extend(filters);
        for (name, tag) in tags {
            if self
                .registered_compilers
                .as_ref()
                .and_then(|registered| registered.get(&name))
                .is_none_or(|registered| !registered.is(&tag))
            {
                self.register_tag(&name, tag)?;
            }
        }
        self.skip_until = end;
        Ok(TokenTree::tag(Tag::PythonNode(PythonNode(Gc::new(node)))))
    }

    /// Whether a tag between `start` and `end` shares parse-time state that
    /// native and Django parsers can't see across an opaque boundary.
    fn shares_parser_state(&self, start: usize, end: usize) -> bool {
        Lexer::new(self.template)
            .skip_while(|token| token.at.0 < start)
            .take_while(|token| token.at.0 < end)
            .any(|token| {
                token.token_type == TokenType::Tag
                    && token
                        .content(self.template)
                        .split_whitespace()
                        .next()
                        .is_some_and(|name| {
                            matches!(
                                name,
                                "extends"
                                    | "block"
                                    | "cycle"
                                    | "resetcycle"
                                    | "partialdef"
                                    | "partial"
                            )
                        })
            })
    }

    pub(in crate::parse) fn register_tag(
        &mut self,
        name: &str,
        tag: Bound<'py, PyAny>,
    ) -> PyResult<()> {
        if let Some(registered) = &mut self.registered_compilers {
            registered.insert(name.to_string(), tag.clone());
        }
        // End markers are recognized even without a matching opening tag. Reading
        // their names does not validate or compile the tag's callable.
        if let Ok(code) = tag.getattr("__code__") {
            let names: Vec<String> = code.getattr("co_freevars")?.extract()?;
            if code.getattr("co_name")?.extract::<String>()? == "compile_func"
                && names.iter().any(|name| name == "func")
                && let Some(index) = names.iter().position(|name| name == "end_name")
            {
                let end_name: String = tag
                    .getattr("__closure__")?
                    .get_item(index)?
                    .getattr("cell_contents")?
                    .extract()?;
                self.external_tags
                    .insert(end_name, TagContext::EndSimpleBlock);
            }
        }
        self.external_tags
            .insert(name.to_string(), TagContext::Unloaded(tag));
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(in crate::parse) fn load_tag(
        &mut self,
        at: At,
        name: &str,
        tag: &Bound<'py, PyAny>,
    ) -> Result<(), PyParseError> {
        if let Some(kind) = stock::compiler(self.py, tag)? {
            self.external_tags
                .insert(name.to_string(), TagContext::Stock(kind));
            return Ok(());
        }
        if let Some(admin) = stock::admin_inclusion(self.py, tag)? {
            let context = self.admin_inclusion_context(name, admin)?;
            self.external_tags.insert(
                name.to_string(),
                TagContext::AdminInclusion {
                    context,
                    template_name: admin.template_name,
                },
            );
            return Ok(());
        }
        let Ok(closure) = tag.getattr("__closure__") else {
            return self.register_opaque(name, tag);
        };
        let tag = if closure.is_none() {
            return self.register_opaque(name, tag);
        } else {
            let tag_code = tag.getattr("__code__")?;
            let closure_names: Vec<String> = tag_code.getattr("co_freevars")?.extract()?;
            // Only Django's generated helpers have the closure layout below.
            // Arbitrary compilers, including closures and callable objects, use
            // the compatible Python parser instead of indexing assumed cells.
            if tag_code.getattr("co_name")?.extract::<String>()? != "compile_func"
                || !COMPILE_FUNC_CELLS
                    .iter()
                    .all(|required| closure_names.iter().any(|name| name == required))
            {
                return self.register_opaque(name, tag);
            }
            let closure_values = closure
                .try_iter()?
                .map(|v| v?.getattr("cell_contents"))
                .collect::<Result<Vec<_>, _>>()?;

            // Django 6.1 validates and removes implicit arguments at registration.
            let normalized_params = self.engine.django_series >= (6, 1);
            // Cells are read by name: their positions follow the sorted free
            // variable names, which a new Django variable would shift.
            let cell = |name: &str| -> PyResult<&Bound<'py, PyAny>> {
                let index = closure_names
                    .iter()
                    .position(|key| key == name)
                    .ok_or_else(|| {
                        pyo3::exceptions::PyValueError::new_err(format!(
                            "Missing tag closure cell: {name}"
                        ))
                    })?;
                Ok(&closure_values[index])
            };
            let defaults_count = get_defaults_count(cell("defaults")?)?;
            let func = cell("func")?.clone();
            let function_name: String = cell("function_name")?.extract()?;
            let kwonly: Vec<String> = cell("kwonly")?.extract()?;
            let kwonly_defaults = get_kwonly_defaults(cell("kwonly_defaults")?)?;
            let params: Vec<String> = cell("params")?.extract()?;
            let takes_context = cell("takes_context")?.is_truthy()?;
            let varargs = !cell("varargs")?.is_none();
            let varkw = !cell("varkw")?.is_none();
            if closure_names.contains(&"filename".to_string()) {
                let mut params = params;
                if takes_context && !normalized_params {
                    if params.first().is_none_or(|name| name != "context") {
                        return Err(ParseError::RequiresContext {
                            name: function_name,
                            at: at.into(),
                        }
                        .into());
                    }
                    params.remove(0);
                }
                TagContext::Inclusion {
                    filename: cell("filename")?.clone(),
                    context: SimpleTagContext {
                        func,
                        function_name,
                        takes_context,
                        params,
                        defaults_count,
                        varargs,
                        kwonly,
                        kwonly_defaults,
                        varkw,
                    },
                }
            } else if closure_names.contains(&"end_name".to_string()) {
                let end_tag_name: String = cell("end_name")?.extract()?;

                let params = if normalized_params {
                    params
                } else {
                    match takes_context {
                        false => {
                            if let Some(param) = params.first()
                                && param == "content"
                            {
                                params.iter().skip(1).cloned().collect()
                            } else {
                                return Err(ParseError::RequiresContent {
                                    name: function_name,
                                    at: at.into(),
                                }
                                .into());
                            }
                        }
                        true => {
                            if let Some([context, content]) = params.first_chunk::<2>()
                                && context == "context"
                                && content == "content"
                            {
                                params.iter().skip(2).cloned().collect()
                            } else {
                                return Err(ParseError::RequiresContextAndContent {
                                    name: function_name,
                                    at: at.into(),
                                }
                                .into());
                            }
                        }
                    }
                };
                // TODO: `end_tag_name already present?
                self.external_tags
                    .insert(end_tag_name.clone(), TagContext::EndSimpleBlock);
                TagContext::SimpleBlock {
                    end_tag_name,
                    context: SimpleTagContext {
                        func,
                        function_name,
                        takes_context,
                        params,
                        defaults_count,
                        varargs,
                        kwonly,
                        kwonly_defaults,
                        varkw,
                    },
                }
            } else {
                let params = match takes_context && !normalized_params {
                    false => params,
                    true => {
                        if let Some(param) = params.first()
                            && param == "context"
                        {
                            params.iter().skip(1).cloned().collect()
                        } else {
                            return Err(ParseError::RequiresContext {
                                name: function_name,
                                at: at.into(),
                            }
                            .into());
                        }
                    }
                };
                TagContext::Simple(SimpleTagContext {
                    func,
                    function_name,
                    takes_context,
                    params,
                    defaults_count,
                    varargs,
                    kwonly,
                    kwonly_defaults,
                    varkw,
                })
            }
        };
        self.external_tags.insert(name.to_string(), tag);
        Ok(())
    }

    /// The call signature `InclusionAdminNode` reads from its function.
    fn admin_inclusion_context(
        &self,
        name: &str,
        admin: stock::AdminInclusion,
    ) -> PyResult<SimpleTagContext<'py>> {
        let func = match admin.func {
            Some(func) => self.py.import(admin.module)?.getattr(func)?,
            None => self
                .py
                .import("django_rusty_templates.stock")?
                .getattr("admin_context")?,
        };
        let spec = self
            .py
            .import("inspect")?
            .getattr("getfullargspec")?
            .call1((&func,))?;
        let mut params: Vec<String> = spec.getattr("args")?.extract()?;
        // Every admin function that takes the context names it first.
        if admin.takes_context {
            params.remove(0);
        }
        Ok(SimpleTagContext {
            function_name: name.to_string(),
            takes_context: admin.takes_context,
            params,
            defaults_count: get_defaults_count(&spec.getattr("defaults")?)?,
            varargs: !spec.getattr("varargs")?.is_none(),
            kwonly: spec.getattr("kwonlyargs")?.extract()?,
            kwonly_defaults: get_kwonly_defaults(&spec.getattr("kwonlydefaults")?)?,
            varkw: !spec.getattr("varkw")?.is_none(),
            func,
        })
    }

    pub(in crate::parse) fn get_tags(
        &self,
        library: &Bound<'py, PyAny>,
    ) -> PyResult<HashMap<String, Bound<'py, PyAny>>> {
        library.getattr(intern!(self.py, "tags"))?.extract()
    }

    pub(in crate::parse) fn get_filters(
        &self,
        library: &Bound<'py, PyAny>,
    ) -> PyResult<HashMap<String, Bound<'py, PyAny>>> {
        library.getattr(intern!(self.py, "filters"))?.extract()
    }
}

fn get_defaults_count(defaults: &Bound<'_, PyAny>) -> PyResult<usize> {
    match defaults.is_none() {
        true => Ok(0),
        false => defaults.len(),
    }
}

fn get_kwonly_defaults(kwonly_defaults: &Bound<'_, PyAny>) -> PyResult<HashSet<String>> {
    match kwonly_defaults.is_none() {
        true => Ok(HashSet::new()),
        false => kwonly_defaults
            .try_iter()?
            .map(|item| item?.extract())
            .collect::<PyResult<_>>(),
    }
}
