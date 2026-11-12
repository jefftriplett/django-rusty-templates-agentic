"""Django parser/node interoperability with native variable rendering."""

from django import VERSION
from django.template import Context, TemplateDoesNotExist
from django.template.backends.django import copy_exception
from django.template.context import make_context
from django.template import defaultfilters
from django.template.base import (
    FilterExpression,
    Node,
    NodeList,
    Origin,
    Parser,
    Template,
    TextNode,
    TokenType,
    VariableNode,
    filter_re,
)
from django.template.loader_tags import BlockNode, ExtendsNode, IncludeNode
from django.template.defaulttags import (
    CommentNode,
    ForNode,
    IfNode,
    LoadNode,
    URLNode,
    do_for,
    do_if,
)
from django.template.library import InclusionNode, SimpleNode

from . import _django_get_template
from .django_rusty_templates import Template as NativeTemplate
from .diagnostics import allows_multiline_tags, make_lexer
from .inheritance import NativeExtendsNode


# Django's own filter functions, whose arguments Rust checks as Django does.
STOCK_FILTERS = dict(defaultfilters.register.filters)

# Django's own control flow compilers. Rust compiles their arguments again
# when it compiles the region around them, so Django can defer its copy.
STOCK_CONTROL_FLOW = {"for": do_for, "if": do_if}


# Syntax added in later Django versions.
_RANGE_SYNTAX = VERSION >= (6, 1)
_EXPONENT_SYNTAX = VERSION >= (6, 0)


def version_specific(contents):
    """Expressions only the version's own Django parser understands."""
    return (_RANGE_SYNTAX and ".." in contents) or (
        _EXPONENT_SYNTAX
        and (
            "e+" in contents or "E+" in contents or "e-" in contents or "E-" in contents
        )
    )


class LazyFilterExpression(FilterExpression):
    """A variable's FilterExpression, parsed on first use.

    Rust compiles the variables in native code from their source, so most
    never need Django's parsed form. Inspection and Django rendering still get
    a real FilterExpression: the first read of its parsed attributes runs
    Django's parser. Only expressions whose filters are all Django's own are
    lazy, so they parse with Django's filters instead of keeping the
    template's filter mapping alive.
    """

    __slots__ = ()

    def __init__(self, token):
        self.token = token

    def __getattr__(self, name):
        if name not in ("filters", "var", "is_var"):
            raise AttributeError(name)
        self.parse()
        return getattr(self, name)

    def parse(self):
        lookup = Parser.__new__(Parser)
        lookup.filters = STOCK_FILTERS
        FilterExpression.__init__(self, self.token, lookup)


class CulpritNode:
    """The node Django's debug hook reports: a token in the full template."""

    def __init__(self, token, origin):
        self.token = token
        self.origin = origin


class NativeVariableNode(VariableNode):
    def __init__(self, node, engine):
        super().__init__(node.filter_expression)
        self.engine = engine
        self._native = None

    @property
    def source(self):
        """The variable as a template of its own, after the loads before it."""
        return self.load_prefix + "{{ " + self.token.contents + " }}"

    @property
    def native(self):
        if self._native is None:
            self._native = self.engine._from_string_fragment(self.source)
        return self._native

    def debug_token(self, error):
        return self.token

    def render(self, context):
        try:
            return self.native._render_fragment(context)
        except Exception as error:
            # The fragment has its own source offsets. Name the node that
            # failed in the complete template, and let Django's node hook
            # annotate the exception once it reaches that template. Inner
            # Django nodes and other templates have already done so.
            if (
                self.engine.debug
                and not hasattr(error, "template_debug")
                and not hasattr(error, "_culprit_node")
            ):
                token = self.debug_token(error)
                if token is not None and token.position is not None:
                    error._culprit_node = CulpritNode(token, self.origin)
            raise


class NativeRegionNode(Node):
    """Render supported native control flow, variables and text in one call."""

    child_nodelists = ("nodelist",)
    render = NativeVariableNode.render

    @property
    def native(self):
        if self._native is None:
            self._native = self.engine._compile_interop_region(
                self.source, self.callbacks, self.origin.template_name, self.origin.name
            )
        return self._native

    def __init__(
        self,
        nodes,
        engine,
        source,
        callbacks=(),
        template_source="",
        compiled=(),
        start=0,
    ):
        # The ids of the variables' FilterExpressions this region compiles.
        self.compiled = compiled
        self.nodelist = NodeList(nodes)
        self.engine = engine
        self.source = source
        self._native = None
        self.callbacks = callbacks
        self.token = nodes[0].token
        self.origin = nodes[0].origin
        self.load_prefix = nodes[0].load_prefix
        self._native_start = start
        self._native_end = nodes[-1]._native_end
        self.template_source = template_source

    def debug_token(self, error):
        """The full template's token at the fragment span Rust recorded."""
        span = getattr(error, "_rusty_token_span", None)
        if span is None:
            return None
        start = span[0] - len(self.load_prefix.encode())
        region_start = self._native_start
        region = self.template_source[region_start : self._native_end].encode()
        if not 0 <= start <= len(region):
            return None
        position = region_start + len(region[:start].decode(errors="ignore"))
        return next(
            (
                token
                for token in make_lexer(
                    self.template_source,
                    allow_multiline_tags=allows_multiline_tags(self.engine),
                ).tokenize()
                if token.position[0] <= position < token.position[1]
            ),
            None,
        )


class NativeParser(Parser):
    def __init__(self, source, engine, origin, lazy=False):
        tokens = make_lexer(
            source, allow_multiline_tags=allows_multiline_tags(engine)
        ).tokenize()
        # Where each token starts in the source, which Django only keeps in
        # debug mode. Kept here so it's freed with the parser.
        self.starts = {token: token.position[0] for token in tokens}
        if not engine.debug:
            for token in tokens:
                token.position = None
        super().__init__(
            tokens,
            engine.template_libraries,
            engine.template_builtins,
            origin,
        )
        self.native_engine = engine
        self.source = source
        self.load_prefix = ""
        self.parse_depth = 0
        self.native_leaf_cache = {}
        self.lazy = lazy
        self.lazy_expressions = []
        self.lazy_support = {}
        self.variable_token = None
        # Lazy for/if arguments, by the tag's token.
        self.lazy_arguments = {}

    def next_token(self):
        token = super().next_token()
        self.variable_token = token if token.token_type is TokenType.VAR else None
        return token

    def add_library(self, lib):
        # Lazy expressions keep the filters in effect where they appeared, so
        # a later {% load %} replaces the mapping instead of changing it.
        self.tags.update(lib.tags)
        self.filters = {**self.filters, **lib.filters}
        self.lazy_support = {}

    def compile_filter(self, token):
        variable = self.variable_token
        self.variable_token = None
        if self.lazy and self.lazy_supported(token):
            if variable is not None:
                lazy = variable.contents is token
                tag = None
            else:
                # A stock {% for %} or {% if %} (including its {% elif %})
                # compiling its own arguments, not a tag in its body.
                command, tag = (
                    self.command_stack[-1] if self.command_stack else ("", None)
                )
                lazy = STOCK_CONTROL_FLOW.get(command, False) is self.tags.get(command)
            if lazy:
                expression = LazyFilterExpression(token)
                self.lazy_expressions.append(expression)
                if tag is not None:
                    self.lazy_arguments.setdefault(tag, []).append(expression)
                return expression
        return super().compile_filter(token)

    def lazy_supported(self, token):
        """Whether Rust checks this variable as Django's parser would.

        Cached per token until the next {% load %} changes the filters."""
        supported = self.lazy_support.get(token)
        if supported is None:
            supported = not version_specific(token) and (
                # Without a filter, there is nothing a library could replace.
                "|" not in token
                or all(
                    name is None
                    or (
                        name in STOCK_FILTERS
                        and self.filters.get(name) is STOCK_FILTERS[name]
                    )
                    for name in (
                        match["filter_name"] for match in filter_re.finditer(token)
                    )
                )
            )
            self.lazy_support[token] = supported
        return supported

    @staticmethod
    def native_node(node):
        kind = type(node)
        if kind is TextNode:
            return node.token.token_type.value == 0
        if kind is ForNode:
            if len(node.loopvars) != 1 or node.loopvars[0] == "forloop":
                return False
            children = (node.nodelist_loop, node.nodelist_empty)
        elif kind is IfNode:
            children = [nodelist for _, nodelist in node.conditions_nodelists]
        else:
            # Inheritance remains visible to Django's block discovery. Other
            # opaque nodes can execute at explicit boundaries inside a Rust
            # region.
            return not isinstance(node, (BlockNode, ExtendsNode))
        for nodelist in children:
            for child in nodelist:
                if not NativeParser.native_node(child):
                    return False
        return True

    def parse(self, parse_until=None):
        self.parse_depth += 1
        try:
            nodes = super().parse(parse_until)
        finally:
            self.parse_depth -= 1
        if self.parse_depth:
            return nodes
        self.inspection_nodes = nodes
        return self.group_nodes(nodes)

    def group_nodes(self, nodes):
        grouped = NodeList()
        grouped.contains_nontext = nodes.contains_nontext
        region = []

        def flush():
            variables = [
                node for node in region if isinstance(node, NativeVariableNode)
            ]
            structural = any(
                type(node) not in (TextNode, NativeVariableNode) for node in region
            )
            if len(variables) >= 2 or structural:
                start = self.starts[region[0].token]
                end = region[-1]._native_end
                source = region[0].load_prefix + self.source[start:end]
                callbacks = []
                compiled = set()
                prefix_bytes = len(region[0].load_prefix.encode())
                # Callbacks use UTF-8 byte offsets into the region source.
                ascii_region = self.source[start:end].isascii()

                def byte_offset(position):
                    if ascii_region:
                        return prefix_bytes + position - start
                    return prefix_bytes + len(self.source[start:position].encode())

                def collect(node):
                    kind = type(node)
                    if kind is NativeVariableNode:
                        compiled.add(id(node.filter_expression))
                        return
                    if kind is NativeRegionNode:
                        compiled.update(node.compiled)
                        return
                    if kind is TextNode:
                        return
                    if kind is ForNode or kind is IfNode:
                        # Rust compiles this tag's arguments with the region.
                        compiled.update(
                            map(id, self.lazy_arguments.get(node.token, ()))
                        )
                        for name in node.child_nodelists:
                            for child in getattr(node, name):
                                collect(child)
                        return
                    node_start = self.starts[node.token]
                    node_end = node._native_end
                    span = node.load_prefix + self.source[node_start:node_end]
                    if type(node) in (
                        SimpleNode,
                        InclusionNode,
                        URLNode,
                        IncludeNode,
                        LoadNode,
                        CommentNode,
                    ) and self.native_leaf_supported(span):
                        return
                    for name in node.child_nodelists:
                        children = getattr(node, name, None)
                        if children is not None:
                            children[:] = self.group_nodes(children)
                    callbacks.append(
                        (byte_offset(node_start), byte_offset(node_end), node)
                    )

                for node in region:
                    collect(node)
                grouped.append(
                    NativeRegionNode(
                        region[:],
                        self.native_engine,
                        source,
                        callbacks,
                        self.source,
                        compiled,
                        start,
                    )
                )
            else:
                grouped.extend(region)
            region.clear()

        for node in nodes:
            # Nodes from tokens a compiler made itself have no known span.
            start = self.starts.get(node.token)
            eligible = start is not None and self.native_node(node)
            if eligible and type(node) is not TextNode:
                span = self.source[start : node._native_end]
                eligible = (
                    # A parent loop may be owned by Django outside this region.
                    "forloop.parentloop" not in span
                    # These expressions require the version-specific Django parser.
                    and not version_specific(span)
                )
            if not eligible:
                flush()
                for name in node.child_nodelists:
                    children = getattr(node, name, None)
                    if children is not None:
                        # IfNode.nodelist is an inspection property, not its
                        # actual branch lists.
                        if type(node) is IfNode:
                            for _, branch in node.conditions_nodelists:
                                branch[:] = self.group_nodes(branch)
                            break
                        children[:] = self.group_nodes(children)
                grouped.append(node)
                continue
            # Do not join spans separated by discarded comments or tags.
            if region and (region[-1]._native_end != start):
                flush()
            region.append(node)
        flush()
        return grouped

    def native_leaf_supported(self, source):
        # Repeated component snippets share a compilation probe within this
        # template only. Include the complete load prefix in the cache key.
        if source not in self.native_leaf_cache:
            self.native_leaf_cache[source] = (
                self.native_engine.explain_template(source, self.origin.template_name)[
                    "mode"
                ]
                == "native"
            )
        return self.native_leaf_cache[source]

    def extend_nodelist(self, nodelist, node, token):
        # A compiler may consume a block or perform lookahead and push a token
        # back. The next remaining token identifies its actual consumed span.
        end = self.starts[self.tokens[-1]] if self.tokens else len(self.source)
        if token.contents.startswith("load ") and token.token_type.value == 2:
            self.load_prefix += "{% " + token.contents + " %}"
        if type(node) is ExtendsNode:
            node.__class__ = NativeExtendsNode
            node.parent_name = ParentExpression(node.parent_name)
        if type(node) is VariableNode and not version_specific(token.contents):
            node = NativeVariableNode(node, self.native_engine)
        node.load_prefix = self.load_prefix
        node._native_end = end
        super().extend_nodelist(nodelist, node, token)


class InspectionNodeList(NodeList):
    """Keep Django's public node indexing while rendering compiled regions."""

    def __init__(self, inspection_nodes, rendering_nodes):
        super().__init__(inspection_nodes)
        self.contains_nontext = inspection_nodes.contains_nontext
        self.rendering_nodes = rendering_nodes

    def render(self, context):
        return self.rendering_nodes.render(context)


class InteropTemplate(Template):
    @staticmethod
    def compile_native_nodes(nodes, compiled=None):
        """Compile native nodes, adding the ids of the variables Rust parsed
        to ``compiled``."""
        for node in nodes:
            kind = type(node)
            if kind is NativeRegionNode:
                # Compile only the maximal regions, not their inspection nodes.
                # A region's source covers its loops and conditions, but not
                # the bodies of opaque callback nodes, which compile on their own.
                node.native
                if compiled is not None:
                    compiled.update(node.compiled)
                # Only needed while compiling; don't keep it with the template.
                node.compiled = ()
                for _, _, callback in node.callbacks:
                    for name in callback.child_nodelists:
                        InteropTemplate.compile_native_nodes(
                            getattr(callback, name, ()), compiled
                        )
            elif kind is NativeVariableNode:
                node.native
                if compiled is not None:
                    compiled.add(id(node.filter_expression))
            else:
                for name in node.child_nodelists:
                    InteropTemplate.compile_native_nodes(
                        getattr(node, name, ()), compiled
                    )

    def compile_nodelist(self):
        parser = NativeParser(self.source, self.engine, self.origin, lazy=True)
        try:
            return self.compile_with(parser)
        except Exception as error:
            if not parser.lazy_expressions:
                self.compile_error(error)
        # A deferred variable may hold the first error Django reports, so
        # compile again with every variable parsed in order.
        parser = NativeParser(self.source, self.engine, self.origin)
        try:
            return self.compile_with(parser)
        except Exception as error:
            self.compile_error(error)

    def compile_with(self, parser):
        nodes = parser.parse()
        self.extra_data = parser.extra_data
        compiled = set() if parser.lazy_expressions else None
        self.compile_native_nodes(nodes, compiled)
        for partial in self.extra_data.get("partials", {}).values():
            self.compile_native_nodes(partial.nodelist, compiled)
        for expression in parser.lazy_expressions:
            # Variables Rust didn't compile are checked by Django now.
            if id(expression) not in compiled:
                expression.parse()
        return InspectionNodeList(getattr(parser, "inspection_nodes", nodes), nodes)

    def compile_error(self, error):
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
        raise error

    def bind_origin(self, origin):
        old_origin = self.origin
        # Rust exposes fresh Origin objects when delegating renders. Equivalent
        # loader/name bindings need no work, even when their identities differ.
        if origin is old_origin or (
            origin == old_origin and origin.template_name == old_origin.template_name
        ):
            return
        self.origin = origin
        for partial in self.extra_data.get("partials", {}).values():
            if partial.origin is old_origin:
                partial.origin = origin
        # Region inspection children already contain the original nodes; a
        # second walk of the public inspection list visits them all again.
        for node in self.nodelist.rendering_nodes.get_nodes_by_type(Node):
            if node.origin is old_origin:
                node.origin = origin

    def render(self, context=None, request=None):
        if not isinstance(context, Context):
            context = make_context(context, request, autoescape=self.engine.autoescape)
        return super().render(context)


def compile_template(source, engine, filename=None, template_name=None):
    return InteropTemplate(
        source,
        origin=Origin(filename or "<unknown source>", template_name=template_name),
        name=template_name,
        engine=engine,
    )


def find_template(engine, name, skip=None):
    # Base loader implements Django's origin skip semantics, including extending
    # the same filename from an earlier directory in the loader search order.
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
                if isinstance(cached, NativeTemplate):
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
        parent = self.expression.resolve(context)
        native = getattr(parent, "template", parent)
        if isinstance(native, NativeTemplate):
            return native if native._is_native else native._django_template
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
    partial = _django_get_template(engine, name)
    parent = engine.get_template(name.partition("#")[0])
    return PartialRenderer(partial, parent.source)
