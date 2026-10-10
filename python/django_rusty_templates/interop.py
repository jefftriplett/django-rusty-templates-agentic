"""Django parser/node interoperability with native variable rendering."""

from django import VERSION
from django.template import Context
from django.template.context import make_context
from django.template.base import DebugLexer, Lexer, Node, Parser, Template, VariableNode
from django.template.loader_tags import ExtendsNode


class NativeVariableNode(VariableNode):
    def __init__(self, node, native):
        super().__init__(node.filter_expression)
        self.native = native
        variables = [node.filter_expression.var]
        variables.extend(
            argument
            for _, arguments in node.filter_expression.filters
            for lookup, argument in arguments
            if lookup
        )
        self.context_names = tuple(
            dict.fromkeys(
                variable.lookups[0]
                for variable in variables
                if getattr(variable, "lookups", None)
            )
        )

    def render(self, context):
        try:
            return self.native._render_fragment(context, self.context_names)
        except Exception as error:
            # The fragment has its own source offsets. Let Django's node hook
            # annotate this exception against the complete inherited template.
            if hasattr(error, "template_debug"):
                del error.template_debug
            raise


class NativeParser(Parser):
    def __init__(self, source, engine, origin):
        super().__init__(
            (DebugLexer if engine.debug else Lexer)(source).tokenize(),
            engine.template_libraries,
            engine.template_builtins,
            origin,
        )
        self.native_engine = engine
        self.load_prefix = ""

    def extend_nodelist(self, nodelist, node, token):
        if token.contents.startswith("load ") and token.token_type.value == 2:
            self.load_prefix += "{% " + token.contents + " %}"
        if isinstance(node, ExtendsNode):
            node.parent_name = ParentExpression(node.parent_name)
        if type(node) is VariableNode and not (
            (VERSION >= (6, 1) and ".." in token.contents)
            or (
                VERSION >= (6, 0)
                and any(
                    exponent in token.contents for exponent in ("e+", "E+", "e-", "E-")
                )
            )
        ):
            fragment = self.load_prefix + "{{ " + token.contents + " }}"
            node = NativeVariableNode(node, self.native_engine.from_string(fragment))
        super().extend_nodelist(nodelist, node, token)


class InteropTemplate(Template):
    def compile_nodelist(self):
        parser = NativeParser(self.source, self.engine, self.origin)
        try:
            nodes = parser.parse()
            self.extra_data = parser.extra_data
            return nodes
        except Exception as error:
            diagnostic = self.engine._syntax_diagnostic(
                self.source,
                None if self.origin.name == "<unknown source>" else self.origin.name,
                self.origin.template_name,
            )
            if diagnostic:
                error.rusty_diagnostic = diagnostic
                error.add_note(diagnostic)
            if self.engine.debug and hasattr(error, "token"):
                error.template_debug = self.get_exception_info(error, error.token)
            raise

    def bind_origin(self, origin):
        old_origin = self.origin
        self.origin = origin
        for partial in self.extra_data.get("partials", {}).values():
            if partial.origin is old_origin:
                partial.origin = origin
        for node in self.nodelist.get_nodes_by_type(Node):
            if node.origin is old_origin:
                node.origin = origin

    def render(self, context=None, request=None):
        if not isinstance(context, Context):
            context = make_context(context, request, autoescape=self.engine.autoescape)
        return super().render(context)


def compile_template(source, engine, filename=None, template_name=None):
    from django.template.base import Origin

    return InteropTemplate(
        source,
        origin=Origin(filename or "<unknown source>", template_name=template_name),
        name=template_name,
        engine=engine,
    )


def find_template(engine, name, skip=None):
    # Base loader implements Django's origin skip semantics, including extending
    # the same filename from an earlier directory in the loader search order.
    from django.template import TemplateDoesNotExist
    from django.template.backends.django import copy_exception

    tried = []
    for loader in engine.template_loaders:
        try:
            key = loader.cache_key(name, skip) if loader.is_cached else None
            if key is not None:
                cached = loader.get_template_cache.get(key)
                if isinstance(cached, type) and issubclass(
                    cached, TemplateDoesNotExist
                ):
                    raise cached(name)
                if isinstance(cached, TemplateDoesNotExist):
                    raise copy_exception(cached)
                if cached is not None and hasattr(cached, "_django_template"):
                    template = cached if cached._is_native else cached._django_template
                    return template, cached.origin
            loader_tried = []
            for origin in loader.get_template_sources(name):
                if skip is not None and origin in skip:
                    tried.append((origin, "Skipped to avoid recursion"))
                    continue
                try:
                    contents = origin.loader.get_contents(origin)
                except TemplateDoesNotExist:
                    tried.append((origin, "Source does not exist"))
                    loader_tried.append((origin.name, "Source does not exist"))
                    continue
                template = engine._compile_loaded_template(contents, origin, name)
                if key is not None:
                    loader._cache_template(key, template)
                    cached = loader.get_template_cache[key]
                    template = cached if cached._is_native else cached._django_template
                    return template, origin
                return template, origin
            if key is not None:
                loader._cache_missing(key, loader_tried)
        except TemplateDoesNotExist as error:
            tried.extend(error.tried or ())
    raise TemplateDoesNotExist(name, tried=tried)


class ParentExpression:
    def __init__(self, expression):
        self.expression = expression

    def __getattr__(self, name):
        return getattr(self.expression, name)

    def resolve(self, context):
        from .django_rusty_templates import Template as NativeTemplate

        parent = self.expression.resolve(context)
        native = getattr(parent, "template", parent)
        if isinstance(native, NativeTemplate):
            return native._django_template
        return parent


class PartialRenderer:
    """Expose a Django partial through the native template render protocol."""

    def __init__(self, partial, source):
        self.partial = partial
        self.source = source
        self.extra_data = {}

    def __getattr__(self, name):
        return getattr(self.partial, name)

    def bind_origin(self, origin):
        # Partials keep the full template's origin for source/debug delegation.
        pass

    def render(self, context=None, request=None):
        if not isinstance(context, Context):
            context = make_context(
                context, request, autoescape=self.partial.engine.autoescape
            )
        return self.partial.render(context)


def compile_partial(name, engine):
    from . import _django_get_template

    partial = _django_get_template(engine, name)
    parent = engine.get_template(name.partition("#")[0])
    return PartialRenderer(partial, parent.source)
