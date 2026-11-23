#!/usr/bin/env python3
# Copyright 2026 ddbsh reimplementation contributors.
# SPDX-License-Identifier: Apache-2.0
"""Golden-suite runner for rddbsh (replaces the CMake/CTest golden project).

Parses the original CTest family files (families/*.txt) verbatim, then runs
each test in file order with CTest pass/fail semantics:

  * PASS_REGULAR_EXPRESSION set  -> pass iff the output matches (exit code ignored)
  * otherwise                    -> pass iff exit code == 0
  * FAIL_REGULAR_EXPRESSION hit  -> fail
  * WILL_FAIL (CMake truthy)     -> invert

`ddbshtest` is emulated exactly like golden-runner.sh: -q is always passed,
stdout+stderr are captured combined, -s sorts with C-locale ordering, and
-T diffs against a golden .exp file.

Usage:
  tests/golden/run-golden.py [--bin target/debug/rddbsh] [--endpoint URL]
                             [--family NAME ...] [--filter REGEX] [-v]

--endpoint sets DDBSH_ENDPOINT_OVERRIDE for the shell and AWS_ENDPOINT_URL_DYNAMODB
for the aws CLI used by ddbsh-wait (e.g. http://localhost:8000 for DynamoDB Local).
"""

import argparse
import difflib
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))

DEFAULT_FAMILIES = [
    "basic", "create-drop", "pitr", "symbol-table", "select", "update", "delete",
    "insert-replace", "composite-gsi", "vector", "transactions", "return-values",
    "test-cleanup", "cross-account", "help", "explain", "show", "global-tables",
]

CMAKE_TRUE = {"1", "ON", "YES", "TRUE", "Y"}


# ---------------------------------------------------------------- CMake lexing

def _read_quoted(text, i):
    """text[i] == '"'. Returns (value, new_index)."""
    i += 1
    out = []
    while i < len(text):
        c = text[i]
        if c == "\\":
            n = text[i + 1] if i + 1 < len(text) else ""
            if n == "\n":
                i += 2  # line continuation
                continue
            mapping = {"n": "\n", "t": "\t", "r": "\r", ";": "\\;"}
            out.append(mapping.get(n, n))
            i += 2
            continue
        if c == '"':
            return "".join(out), i + 1
        out.append(c)
        i += 1
    raise ValueError("unterminated quoted argument")


def parse_cmake(text):
    """Yield (command_name, [args]) for every command invocation."""
    i = 0
    n = len(text)
    while i < n:
        c = text[i]
        if c == "#":
            while i < n and text[i] != "\n":
                i += 1
            continue
        if c.isspace():
            i += 1
            continue
        m = re.compile(r"[A-Za-z_][A-Za-z0-9_]*").match(text, i)
        if not m:
            raise ValueError("unexpected character %r at %d" % (c, i))
        name = m.group(0)
        i = m.end()
        while i < n and text[i] in " \t":
            i += 1
        if i >= n or text[i] != "(":
            raise ValueError("expected '(' after %s" % name)
        i += 1
        args = []
        depth = 0
        while i < n:
            c = text[i]
            if c.isspace():
                i += 1
            elif c == "#":
                while i < n and text[i] != "\n":
                    i += 1
            elif c == '"':
                val, i = _read_quoted(text, i)
                args.append(("q", val))
            elif c == "(":
                depth += 1
                i += 1
            elif c == ")":
                if depth == 0:
                    i += 1
                    break
                depth -= 1
                i += 1
            else:
                j = i
                buf = []
                while j < n and not text[j].isspace() and text[j] not in '()"#':
                    if text[j] == "\\" and j + 1 < n:
                        buf.append(text[j + 1])
                        j += 2
                        continue
                    buf.append(text[j])
                    j += 1
                args.append(("u", "".join(buf)))
                i = j
        yield name, args


def expand(value, variables):
    return re.sub(r"\$\{([A-Za-z0-9_]+)\}", lambda m: variables.get(m.group(1), ""), value)


def split_list(value):
    """Split a CMake list on unescaped ';' (escaped '\\;' stays as ';')."""
    parts = re.split(r"(?<!\\);", value)
    return [p.replace("\\;", ";") for p in parts]


# ------------------------------------------------------------- test discovery

class Test:
    def __init__(self, name, command, family):
        self.name = name
        self.command = command
        self.family = family
        self.props = {}


def load_tests(families, variables):
    tests = []
    by_name = {}
    for family in families:
        path = os.path.join(HERE, "families", family + ".txt")
        with open(path) as fh:
            text = fh.read()
        for cmd, raw in parse_cmake(text):
            args = []
            for kind, val in raw:
                val = expand(val, variables)
                if kind == "u":
                    args.extend(split_list(val))
                else:
                    args.append(val.replace("\\;", ";"))
            lc = cmd.lower()
            if lc == "add_test":
                name = args[args.index("NAME") + 1]
                command = args[args.index("COMMAND") + 1:]
                t = Test(name, command, family)
                tests.append(t)
                by_name[name] = t
            elif lc == "set_property":
                # set_property(TEST <names...> PROPERTY <prop> <values...>)
                # CMake restarts property parsing at every PROPERTY keyword,
                # so `PROPERTY WILL_FAIL PROPERTY PASS_REGULAR_EXPRESSION "x"`
                # sets only PASS_REGULAR_EXPRESSION.
                pi = args.index("PROPERTY")
                names = args[1:pi]
                last = len(args) - 1 - args[::-1].index("PROPERTY")
                prop = args[last + 1]
                values = args[last + 2:]
                for nm in names:
                    if nm in by_name:
                        by_name[nm].props[prop] = values
            elif lc == "set_tests_properties":
                pi = args.index("PROPERTIES")
                names = args[:pi]
                rest = args[pi + 1:]
                for k in range(0, len(rest) - 1, 2):
                    for nm in names:
                        if nm in by_name:
                            by_name[nm].props[rest[k]] = split_list(rest[k + 1])
            # other commands (e.g. set()) are ignored
    return tests


# ------------------------------------------------------------------ execution

def c_sort(data):
    if not data:
        return data
    lines = data.split(b"\n")
    trailing = lines and lines[-1] == b""
    if trailing:
        lines = lines[:-1]
    lines.sort()
    return b"\n".join(lines) + (b"\n" if trailing else b"")


IGNORE_ORDER = False


def run_ddbshtest(argv, shell_bin, env):
    """Emulate golden-runner.sh. Returns (exit_code, output_text)."""
    cmd = file_ = golden = record = None
    sort_output = abort = False
    i = 0
    while i < len(argv):
        a = argv[i]
        if a in ("-c", "-f", "-T", "-R"):
            v = argv[i + 1]
            i += 2
            if a == "-c":
                cmd = v
            elif a == "-f":
                file_ = v
            elif a == "-T":
                golden = v
            else:
                record = v
            continue
        if a.startswith("-") and len(a) > 1:
            for ch in a[1:]:
                if ch == "s":
                    sort_output = True
                elif ch == "e":
                    abort = True
        i += 1
    shell_args = [shell_bin, "-q"] + (["-e"] if abort else [])
    if cmd is not None:
        shell_args += ["-c", cmd]
    elif file_ is not None:
        shell_args += ["-f", file_]
    else:
        return 2, "golden-runner: neither -c nor -f supplied\n"
    proc = subprocess.run(shell_args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env)
    out = proc.stdout
    if sort_output:
        out = c_sort(out)
    text = out.decode("utf-8", "replace")
    if golden is not None:
        if not os.path.isfile(golden):
            return 1, text + "golden-runner: golden not found: %s\n" % golden
        with open(golden, "rb") as fh:
            want = fh.read().decode("utf-8", "replace")
        if want == text:
            return 0, text
        if IGNORE_ORDER and sorted(want.splitlines()) == sorted(text.splitlines()):
            return 0, text
        diff = "".join(difflib.unified_diff(
            want.splitlines(True), text.splitlines(True), golden, "actual"))
        return 1, text + diff + "golden-runner: output did not match golden %s\n" % golden
    if record is not None:
        with open(record, "wb") as fh:
            fh.write(out)
        return 0, text
    return proc.returncode, text


def run_test(t, shell_bin, env):
    prog = t.command[0]
    if prog == "ddbshtest":
        rc, out = run_ddbshtest(t.command[1:], shell_bin, env)
    else:
        proc = subprocess.run(t.command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env)
        rc, out = proc.returncode, proc.stdout.decode("utf-8", "replace")
    pass_re = t.props.get("PASS_REGULAR_EXPRESSION")
    fail_re = t.props.get("FAIL_REGULAR_EXPRESSION")
    if pass_re:
        ok = any(re.search(p, out, re.M) for p in pass_re)
    else:
        ok = rc == 0
    if fail_re and any(re.search(p, out, re.M) for p in fail_re):
        ok = False
    will_fail = t.props.get("WILL_FAIL")
    if will_fail and ";".join(will_fail).upper() in CMAKE_TRUE:
        ok = not ok
    return ok, rc, out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", default=os.path.join(HERE, "..", "..", "target", "debug", "rddbsh"))
    ap.add_argument("--endpoint", default=None)
    ap.add_argument("--family", action="append")
    ap.add_argument("--filter", default=None)
    ap.add_argument("--backend-regex", default=r"Backend: ")
    ap.add_argument("-v", "--verbose", action="store_true")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--ignore-order", action="store_true",
                    help="accept -T output that matches the golden up to line order "
                         "(for backends whose scan order differs, e.g. DynamoDB Local)")
    args = ap.parse_args()

    global IGNORE_ORDER
    IGNORE_ORDER = args.ignore_order
    shell_bin = os.path.abspath(args.bin)
    variables = {
        "GOLDEN_DIR": HERE,
        "RD": os.path.join(HERE, "results"),
        "DATA": os.path.join(HERE, "data"),
        "DDBSH_GOLDEN_BACKEND_REGEX": args.backend_regex,
        "CMAKE_CURRENT_SOURCE_DIR": HERE,
    }
    families = args.family or DEFAULT_FAMILIES
    tests = load_tests(families, variables)
    if args.filter:
        rx = re.compile(args.filter)
        tests = [t for t in tests if rx.search(t.name)]
    if args.list:
        for t in tests:
            print(t.family, t.name)
        return 0

    env = dict(os.environ)
    if args.endpoint:
        env["DDBSH_ENDPOINT_OVERRIDE"] = args.endpoint
        env["AWS_ENDPOINT_URL_DYNAMODB"] = args.endpoint

    failed = []
    for idx, t in enumerate(tests, 1):
        ok, rc, out = run_test(t, shell_bin, env)
        status = "Passed" if ok else "***Failed"
        print("%4d/%d %-50s %s" % (idx, len(tests), t.name, status), flush=True)
        if not ok:
            failed.append(t.name)
            if args.verbose:
                sys.stdout.write("".join("    | " + l + "\n" for l in out.splitlines()))
    print()
    print("%d%% tests passed, %d tests failed out of %d" % (
        (100 * (len(tests) - len(failed)) // max(len(tests), 1)), len(failed), len(tests)))
    for name in failed:
        print("  FAILED:", name)
    return 0 if not failed else 1


if __name__ == "__main__":
    sys.exit(main())
