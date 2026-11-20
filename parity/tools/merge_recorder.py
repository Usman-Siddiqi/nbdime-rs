"""pytest plugin: record inputs/outputs of nbdime merge entry points while upstream tests run."""
import copy, json, os
import nbdime.merging.generic as g
import nbdime.merging.decisions as d
import nbdime.merging.notebooks as n
import nbdime.merging as m

OUT = os.environ["REC_OUT"]
_fh = open(OUT, "w")
_depth = [0]


def _plain(v):
    return json.loads(json.dumps(v, default=lambda o: repr(o)))


def wrap(name, fn, argnames):
    def w(*args, **kwargs):
        top = _depth[0] == 0
        rec = None
        if top:
            try:
                bound = dict(zip(argnames, args)); bound.update(kwargs)
                rec = {k: _plain(copy.deepcopy(v)) if k != "args" else _ns(v) for k, v in bound.items()}
            except Exception as e:
                rec = None
        _depth[0] += 1
        try:
            res = fn(*args, **kwargs)
            ok = True
        except Exception as e:
            res, ok = "%s: %s" % (type(e).__name__, e), False
            raise
        finally:
            _depth[0] -= 1
            if top and rec is not None:
                try:
                    _fh.write(json.dumps({"fn": name, "args": rec, "ok": ok, "result": _plain(res)}) + "\n")
                    _fh.flush()
                except Exception:
                    pass
        return res
    return w


def _ns(a):
    if a is None:
        return None
    return {k: v for k, v in vars(a).items() if isinstance(v, (str, int, float, bool, type(None), list))}


g.decide_merge = m.decide_merge = wrap("decide_merge", g.decide_merge, ["base", "local", "remote", "strategies"])
g.decide_merge_with_diff = wrap("decide_merge_with_diff", g.decide_merge_with_diff, ["base", "local", "remote", "local_diff", "remote_diff", "strategies"])
d.apply_decisions = m.apply_decisions = wrap("apply_decisions", d.apply_decisions, ["base", "decisions"])
n.decide_notebook_merge = wrap("decide_notebook_merge", n.decide_notebook_merge, ["base", "local", "remote", "args"])
n.merge_notebooks = m.merge_notebooks = wrap("merge_notebooks", n.merge_notebooks, ["base", "local", "remote", "args"])
