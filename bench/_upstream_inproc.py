"""Times nbdime.diff_notebooks in-process. Prints seconds as JSON. Run with the oracle venv."""
import json
import sys
import time
import warnings

warnings.simplefilter("ignore")
import nbformat  # noqa: E402
from nbdime import diff_notebooks  # noqa: E402

a = nbformat.read(sys.argv[1], as_version=4)
b = nbformat.read(sys.argv[2], as_version=4)
t = time.perf_counter()
diff_notebooks(a, b)
print(json.dumps({"seconds": time.perf_counter() - t}))
