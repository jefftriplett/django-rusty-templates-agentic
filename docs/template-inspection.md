# Native template inspection

`Template.nodelist` exposes real Django nodes for tools that index nodes, inspect
their attributes, or call `get_nodes_by_type()`. Existing tests and Django's
template tooling are consumers. This view is separate from the Rust rendering
tree: inspecting a native template never changes its rendering engine.

Flat templates containing text, comments, and variables build their inspection
nodes directly. They use Django's filter-expression compiler to preserve the
public `VariableNode.filter_expression` interface, but do not invoke
`Parser.parse()`. Tokens and origins match Django, including positioned tokens
when DEBUG is enabled.

Templates containing block tags retain a full Django compatibility parse for
inspection. Stateful tag compilers and arbitrary library node attributes cannot
be reconstructed reliably from generic proxies. The resulting inspection view
is cached once per template object; repeated inspection and ordinary rendering
do not reparse it. Uncached loader calls create new template objects, so tooling
that inspects every object still pays this cost once for each object.

Native inheritance discovers blocks through `_inheritance_blocks`, and debug
metadata uses the source directly. Neither needs `nodelist`. Mixed templates
already own Django nodes and expose that existing view. The explicit
`_django_template` compatibility adapter remains distinct from inspection.

Run `scripts/benchmark_inspection.py` with a release extension to compare
compilation plus ordinary rendering, compilation plus first inspection and
rendering, and warm rendering after inspection. Flat and block fixtures are
reported separately; these synthetic timings are not HTTP request benchmarks.
