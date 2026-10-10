"""Django parser/node interoperability with native variable rendering."""

from django.template import Context
from django.template.context import make_context
from django.template.base import DebugLexer, Node, Parser, Template, VariableNode
from django.template.loader_tags import ExtendsNode


class NativeVariableNode(VariableNode):
    def __init__(self, node, native):
        super().__init__(node.filter_expression)
        self.native = native

    def render(self, context):
        try:
            return self.native.render(context)
        except Exception as error:
            # The fragment has its own source offsets. Let Django's node hook
            # annotate this exception against the complete inherited template.
            if hasattr(error, "template_debug"):
                del error.template_debug
            raise


class NativeParser(Parser):
    def __init__(self, source, engine, origin):
        super().__init__(
            DebugLexer(source).tokenize(),
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
        if type(node) is VariableNode:
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
            if self.engine.debug and hasattr(error, "token"):
                error.template_debug = self.get_exception_info(error, error.token)
            raise

    def bind_origin(self, origin):
        old_origin = self.origin
        self.origin = origin
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

    tried = []
    for loader in engine.template_loaders:
        try:
            key = loader.cache_key(name, skip) if loader.is_cached else None
            if key is not None:
                cached = loader.get_template_cache.get(key)
                if cached is not None and hasattr(cached, "_django_template"):
                    return cached._django_template, cached.origin
            for origin in loader.get_template_sources(name):
                if skip is not None and origin in skip:
                    tried.append((origin, "Skipped to avoid recursion"))
                    continue
                try:
                    contents = origin.loader.get_contents(origin)
                except TemplateDoesNotExist:
                    tried.append((origin, "Source does not exist"))
                    continue
                template = InteropTemplate(contents, origin, name, engine)
                if key is not None:
                    loader._cache_template(key, template)
                return template, origin
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
