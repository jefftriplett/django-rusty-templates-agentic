"""Django parser/node interoperability with native variable rendering."""

from django import VERSION
from django.template import Context
from django.template.context import make_context
from django.template.base import (
    DebugLexer,
    Node,
    NodeList,
    Parser,
    Template,
    TextNode,
    VariableNode,
)
from django.template.loader_tags import BlockNode, ExtendsNode, IncludeNode
from django.template.defaulttags import CommentNode, ForNode, IfNode, LoadNode, URLNode
from django.template.library import InclusionNode, SimpleNode


class NativeVariableNode(VariableNode):
    def __init__(self, node, engine, source):
        super().__init__(node.filter_expression)
        self.engine = engine
        self.source = source
        self._native = None

    @property
    def native(self):
        if self._native is None:
            self._native = self.engine.from_string(self.source)
        return self._native

    def render(self, context):
        try:
            return self.native._render_fragment(context)
        except Exception as error:
            # The fragment has its own source offsets. Let Django's node hook
            # annotate this exception against the complete inherited template.
            if hasattr(error, "template_debug"):
                del error.template_debug
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

    def __init__(self, nodes, engine, source, callbacks=()):
        self.nodelist = NodeList(nodes)
        self.engine = engine
        self.source = source
        self._native = None
        self.callbacks = callbacks
        self.token = nodes[0].token
        self.origin = nodes[0].origin
        self.load_prefix = nodes[0].load_prefix
        self._native_end = nodes[-1]._native_end


class NativeParser(Parser):
    def __init__(self, source, engine, origin):
        tokens = DebugLexer(source).tokenize()
        for token in tokens:
            token._native_position = token.position
            if not engine.debug:
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

    def next_token(self):
        token = super().next_token()
        self.last_token = token
        return token

    @staticmethod
    def native_node(node):
        if isinstance(node, (NativeVariableNode, NativeRegionNode)):
            return True
        if type(node) is TextNode:
            return node.token.token_type.value == 0
        if type(node) in (ForNode, IfNode):
            if type(node) is ForNode and (
                len(node.loopvars) != 1 or node.loopvars[0] == "forloop"
            ):
                return False
            return all(
                NativeParser.native_node(child)
                for name in node.child_nodelists
                for child in getattr(node, name)
            )
        # Inheritance remains visible to Django's block discovery. Other
        # opaque nodes can execute at explicit boundaries inside a Rust region.
        return type(node) not in (BlockNode, ExtendsNode)

    def parse(self, parse_until=None):
        self.parse_depth += 1
        try:
            nodes = super().parse(parse_until)
        finally:
            self.parse_depth -= 1
        if self.native_engine.debug or self.parse_depth:
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
                start = region[0].token._native_position[0]
                end = region[-1]._native_end
                source = region[0].load_prefix + self.source[start:end]
                callbacks = []
                prefix_bytes = len(region[0].load_prefix.encode())
                offsets = [prefix_bytes]
                for character in self.source[start:end]:
                    offsets.append(offsets[-1] + len(character.encode()))

                def collect(node):
                    if (
                        isinstance(node, (NativeVariableNode, NativeRegionNode))
                        or type(node) is TextNode
                    ):
                        return
                    if type(node) in (ForNode, IfNode):
                        for name in node.child_nodelists:
                            for child in getattr(node, name):
                                collect(child)
                        return
                    node_start = node.token._native_position[0]
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
                        (offsets[node_start - start], offsets[node_end - start], node)
                    )

                for node in region:
                    collect(node)
                grouped.append(
                    NativeRegionNode(region[:], self.native_engine, source, callbacks)
                )
            else:
                grouped.extend(region)
            region.clear()

        for node in nodes:
            span = self.source[node.token._native_position[0] : node._native_end]
            eligible = (
                getattr(node.token, "_native_position", None) is not None
                and self.native_node(node)
                # A parent loop may be owned by Django outside this region.
                and "forloop.parentloop" not in span
                # These expressions require the version-specific Django parser.
                and not (VERSION >= (6, 1) and ".." in span)
                and not (
                    VERSION >= (6, 0)
                    and any(exponent in span for exponent in ("e+", "E+", "e-", "E-"))
                )
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
            if region and (region[-1]._native_end != node.token._native_position[0]):
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
        end = self.tokens[-1]._native_position[0] if self.tokens else len(self.source)
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
            node = NativeVariableNode(node, self.native_engine, fragment)
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
    def compile_native_nodes(nodes):
        for node in nodes:
            if isinstance(node, (NativeVariableNode, NativeRegionNode)):
                # Compile only the maximal regions, not their inspection nodes.
                node.native
                if isinstance(node, NativeRegionNode):
                    for _, _, callback in node.callbacks:
                        for name in callback.child_nodelists:
                            InteropTemplate.compile_native_nodes(
                                getattr(callback, name, ())
                            )
            else:
                for name in node.child_nodelists:
                    InteropTemplate.compile_native_nodes(getattr(node, name, ()))

    def compile_nodelist(self):
        parser = NativeParser(self.source, self.engine, self.origin)
        try:
            nodes = parser.parse()
            self.extra_data = parser.extra_data
            self.compile_native_nodes(nodes)
            for partial in self.extra_data.get("partials", {}).values():
                self.compile_native_nodes(partial.nodelist)
            return InspectionNodeList(getattr(parser, "inspection_nodes", nodes), nodes)
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
        for node in self.nodelist.rendering_nodes.get_nodes_by_type(
            Node
        ) + self.nodelist.get_nodes_by_type(Node):
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
    from .django_rusty_templates import Template as NativeTemplate

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
