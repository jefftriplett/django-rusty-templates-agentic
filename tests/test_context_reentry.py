"""Python code run while the context is locked can use the context again
(#372). Each case runs in a subprocess so a deadlock fails instead of hanging."""

import subprocess
import sys

REPLACED_VALUE_DEL = """
import django
from django.conf import settings
settings.configure()
django.setup()
from django_rusty_templates import Engine

class Included:
    @staticmethod
    def render(context):
        class Value:
            def __del__(self):
                context["z"] = "from __del__"
        context["x"] = Value()
        context["x"] = "replaced"
        return context["z"]

output = Engine().from_string("{% include tmpl %}").render({"tmpl": Included()})
assert output == "from __del__", output
"""


def test_replaced_value_del_can_write_to_the_context():
    subprocess.run([sys.executable, "-c", REPLACED_VALUE_DEL], check=True, timeout=60)
