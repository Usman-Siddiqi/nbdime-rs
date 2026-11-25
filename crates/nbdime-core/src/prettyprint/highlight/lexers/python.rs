//! Pygments' `PythonLexer` (Python 3.x source).

use std::fmt::Write;
use std::sync::OnceLock;

use crate::prettyprint::highlight::lexer::{
    by_groups, by_groups_to, default, rule, rule_to, token, using_this, words_rule, LexerDef,
    NewState, RegexLexer, RuleDef,
};
use crate::prettyprint::highlight::pyre::Flags;
use crate::prettyprint::highlight::token::TokenType as T;
use crate::prettyprint::py_tables::{XID_CONTINUE, XID_START};

/// The aliases `PythonLexer` is registered under.
pub(in crate::prettyprint::highlight) const ALIASES: [&str; 8] = [
    "python", "py", "sage", "python3", "py3", "bazel", "starlark", "pyi",
];

/// `keyword.kwlist` of the reference Python (3.13) without the capitalised names.
const KEYWORDS: [&str; 32] = [
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda",
    "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield",
];

pub(in crate::prettyprint::highlight) fn lexer() -> &'static RegexLexer {
    static LEXER: OnceLock<RegexLexer> = OnceLock::new();
    LEXER.get_or_init(|| RegexLexer::new(&definition()))
}

/// The body of a character class for the code points in a range table.
fn class_body(table: &[u32]) -> String {
    let mut body = String::new();
    for pair in table.chunks_exact(2) {
        let _ = write!(body, "\\U{:08x}-\\U{:08x}", pair[0], pair[1]);
    }
    body
}

/// `innerstring_rules(ttype)`.
fn inner_string_rules(ttype: T) -> Vec<RuleDef> {
    vec![
        // the old style '%s' % (...) string formatting (still valid in Py3)
        rule(
            r"%(\(\w+\))?[-#0 +]*([0-9]+|[*])?(\.([0-9]+|[*]))?[hlL]?[E-GXc-giorsaux%]",
            T::StringInterpol,
        ),
        // the new style '{}'.format(...) string formatting
        rule(
            r"\{((\w+)((\.\w+)|(\[[^\]]+\]))*)?(\![sra])?(\:(.?[<>=\^])?[-+ ]?#?0?(\d+)?,?(\.\d+)?[E-GXb-gnosx%]?)?\}",
            T::StringInterpol,
        ),
        // backslashes, quotes and formatting signs must be parsed one at a time
        rule(r#"[^\\'"%{\n]+"#, ttype),
        rule(r#"['"\\]"#, ttype),
        // unhandled string formatting sign
        rule(r"%|(\{{1,2})", ttype),
    ]
}

/// `fstring_rules(ttype)`.
fn f_string_rules(ttype: T) -> Vec<RuleDef> {
    vec![
        rule(r"\}", T::StringInterpol),
        rule_to(
            r"\{",
            T::StringInterpol,
            NewState::State("expr-inside-fstring"),
        ),
        rule(r#"[^\\'"{}\n]+"#, ttype),
        rule(r#"['"\\]"#, ttype),
    ]
}

fn definition() -> LexerDef {
    use RuleDef::Include;
    let uni_name = format!(
        "[{}][{}]*",
        class_body(&XID_START),
        class_body(&XID_CONTINUE)
    );
    let soft_keywords = format!(
        r"(^[ \t]*)(match|case)\b(?![ \t]*(?:[:,;=^&|@~)\]}}]|(?:{})\b))",
        KEYWORDS.join("|")
    );
    let ws = |a, b| vec![token(a), token(b)];

    let string_start = |pattern: &str, affix: T, quote: T, new_state: NewState| {
        by_groups_to(pattern, vec![token(affix), token(quote)], new_state)
    };

    let states = vec![
        (
            "root",
            vec![
                rule(r"\n", T::Whitespace),
                by_groups(
                    r#"^(\s*)([rRuUbB]{,2})("""[\s\S]*?""")"#,
                    vec![
                        token(T::Whitespace),
                        token(T::StringAffix),
                        token(T::StringDoc),
                    ],
                ),
                by_groups(
                    r"^(\s*)([rRuUbB]{,2})('''[\s\S]*?''')",
                    vec![
                        token(T::Whitespace),
                        token(T::StringAffix),
                        token(T::StringDoc),
                    ],
                ),
                rule(r"\A#!.+$", T::CommentHashbang),
                rule(r"#.*$", T::CommentSingle),
                rule(r"\\\n", T::Text),
                rule(r"\\", T::Text),
                Include("keywords"),
                Include("soft-keywords"),
                by_groups_to(
                    r"(def)((?:\s|\\\s)+)",
                    ws(T::Keyword, T::Whitespace),
                    NewState::State("funcname"),
                ),
                by_groups_to(
                    r"(class)((?:\s|\\\s)+)",
                    ws(T::Keyword, T::Whitespace),
                    NewState::State("classname"),
                ),
                by_groups_to(
                    r"^(lazy)((?:\s|\\\s)+)(from)((?:\s|\\\s)+)",
                    vec![
                        token(T::KeywordNamespace),
                        token(T::Whitespace),
                        token(T::KeywordNamespace),
                        token(T::Whitespace),
                    ],
                    NewState::State("fromimport"),
                ),
                by_groups_to(
                    r"^(lazy)((?:\s|\\\s)+)(import)((?:\s|\\\s)+)",
                    vec![
                        token(T::KeywordNamespace),
                        token(T::Whitespace),
                        token(T::KeywordNamespace),
                        token(T::Whitespace),
                    ],
                    NewState::State("import"),
                ),
                by_groups_to(
                    r"(from)((?:\s|\\\s)+)",
                    ws(T::KeywordNamespace, T::Whitespace),
                    NewState::State("fromimport"),
                ),
                by_groups_to(
                    r"(import)((?:\s|\\\s)+)",
                    ws(T::KeywordNamespace, T::Whitespace),
                    NewState::State("import"),
                ),
                Include("expr"),
            ],
        ),
        (
            "expr",
            vec![
                // raw f-strings and t-strings
                string_start(
                    r#"(?i)(r[ft]|[ft]r)(""")"#,
                    T::StringAffix,
                    T::StringDouble,
                    NewState::Combined(&["rfstringescape", "tdqf"]),
                ),
                string_start(
                    r"(?i)(r[ft]|[ft]r)(''')",
                    T::StringAffix,
                    T::StringSingle,
                    NewState::Combined(&["rfstringescape", "tsqf"]),
                ),
                string_start(
                    r#"(?i)(r[ft]|[ft]r)(")"#,
                    T::StringAffix,
                    T::StringDouble,
                    NewState::Combined(&["rfstringescape", "dqf"]),
                ),
                string_start(
                    r"(?i)(r[ft]|[ft]r)(')",
                    T::StringAffix,
                    T::StringSingle,
                    NewState::Combined(&["rfstringescape", "sqf"]),
                ),
                // non-raw f-strings and t-strings
                string_start(
                    r#"([fFtT])(""")"#,
                    T::StringAffix,
                    T::StringDouble,
                    NewState::Combined(&["fstringescape", "tdqf"]),
                ),
                string_start(
                    r"([fFtT])(''')",
                    T::StringAffix,
                    T::StringSingle,
                    NewState::Combined(&["fstringescape", "tsqf"]),
                ),
                string_start(
                    r#"([fFtT])(")"#,
                    T::StringAffix,
                    T::StringDouble,
                    NewState::Combined(&["fstringescape", "dqf"]),
                ),
                string_start(
                    r"([fFtT])(')",
                    T::StringAffix,
                    T::StringSingle,
                    NewState::Combined(&["fstringescape", "sqf"]),
                ),
                // raw bytes and strings
                string_start(
                    r#"(?i)(rb|br|r)(""")"#,
                    T::StringAffix,
                    T::StringDouble,
                    NewState::State("tdqs"),
                ),
                string_start(
                    r"(?i)(rb|br|r)(''')",
                    T::StringAffix,
                    T::StringSingle,
                    NewState::State("tsqs"),
                ),
                string_start(
                    r#"(?i)(rb|br|r)(")"#,
                    T::StringAffix,
                    T::StringDouble,
                    NewState::State("dqs"),
                ),
                string_start(
                    r"(?i)(rb|br|r)(')",
                    T::StringAffix,
                    T::StringSingle,
                    NewState::State("sqs"),
                ),
                // non-raw strings
                string_start(
                    r#"([uU]?)(""")"#,
                    T::StringAffix,
                    T::StringDouble,
                    NewState::Combined(&["stringescape", "tdqs"]),
                ),
                string_start(
                    r"([uU]?)(''')",
                    T::StringAffix,
                    T::StringSingle,
                    NewState::Combined(&["stringescape", "tsqs"]),
                ),
                string_start(
                    r#"([uU]?)(")"#,
                    T::StringAffix,
                    T::StringDouble,
                    NewState::Combined(&["stringescape", "dqs"]),
                ),
                string_start(
                    r"([uU]?)(')",
                    T::StringAffix,
                    T::StringSingle,
                    NewState::Combined(&["stringescape", "sqs"]),
                ),
                // non-raw bytes
                string_start(
                    r#"([bB])(""")"#,
                    T::StringAffix,
                    T::StringDouble,
                    NewState::Combined(&["bytesescape", "tdqs"]),
                ),
                string_start(
                    r"([bB])(''')",
                    T::StringAffix,
                    T::StringSingle,
                    NewState::Combined(&["bytesescape", "tsqs"]),
                ),
                string_start(
                    r#"([bB])(")"#,
                    T::StringAffix,
                    T::StringDouble,
                    NewState::Combined(&["bytesescape", "dqs"]),
                ),
                string_start(
                    r"([bB])(')",
                    T::StringAffix,
                    T::StringSingle,
                    NewState::Combined(&["bytesescape", "sqs"]),
                ),
                rule(r"[^\S\n]+", T::Text),
                Include("numbers"),
                rule(r"!=|==|<<|>>|:=|[-~+/*%=<>&^|.]", T::Operator),
                rule(r"[]{}:(),;[]", T::Punctuation),
                rule(r"(in|is|and|or|not)\b", T::OperatorWord),
                Include("expr-keywords"),
                Include("builtins"),
                Include("magicfuncs"),
                Include("magicvars"),
                Include("name"),
            ],
        ),
        (
            "expr-inside-fstring",
            vec![
                rule_to(
                    r"[{([]",
                    T::Punctuation,
                    NewState::State("expr-inside-fstring-inner"),
                ),
                // without format specifier
                rule_to(r"(=\s*)?(\![sraf])?\}", T::StringInterpol, NewState::Pop(1)),
                // with format specifier
                rule_to(r"(=\s*)?(\![sraf])?:", T::StringInterpol, NewState::Pop(1)),
                rule(r"\s+", T::Whitespace),
                Include("expr"),
            ],
        ),
        (
            "expr-inside-fstring-inner",
            vec![
                rule_to(
                    r"[{([]",
                    T::Punctuation,
                    NewState::State("expr-inside-fstring-inner"),
                ),
                rule_to(r"[])}]", T::Punctuation, NewState::Pop(1)),
                rule(r"\s+", T::Whitespace),
                Include("expr"),
            ],
        ),
        (
            "expr-keywords",
            vec![
                words_rule(
                    &[
                        "async for",
                        "await",
                        "else",
                        "for",
                        "if",
                        "lambda",
                        "yield",
                        "yield from",
                    ],
                    "",
                    r"\b",
                    T::Keyword,
                ),
                words_rule(&["True", "False", "None"], "", r"\b", T::KeywordConstant),
            ],
        ),
        (
            "keywords",
            vec![
                words_rule(
                    &[
                        "assert",
                        "async",
                        "await",
                        "break",
                        "continue",
                        "del",
                        "elif",
                        "else",
                        "except",
                        "finally",
                        "for",
                        "global",
                        "if",
                        "lambda",
                        "pass",
                        "raise",
                        "nonlocal",
                        "return",
                        "try",
                        "while",
                        "yield",
                        "yield from",
                        "as",
                        "with",
                    ],
                    "",
                    r"\b",
                    T::Keyword,
                ),
                words_rule(&["True", "False", "None"], "", r"\b", T::KeywordConstant),
            ],
        ),
        (
            "soft-keywords",
            vec![by_groups_to(
                &soft_keywords,
                ws(T::Text, T::Keyword),
                NewState::State("soft-keywords-inner"),
            )],
        ),
        (
            "soft-keywords-inner",
            vec![
                // optional `_` keyword
                by_groups(
                    r"(\s+)([^\n_]*)(_\b)",
                    vec![token(T::Whitespace), using_this(), token(T::Keyword)],
                ),
                default(NewState::Pop(1)),
            ],
        ),
        (
            "builtins",
            vec![
                words_rule(
                    &[
                        "__import__",
                        "abs",
                        "aiter",
                        "all",
                        "any",
                        "bin",
                        "bool",
                        "bytearray",
                        "breakpoint",
                        "bytes",
                        "callable",
                        "chr",
                        "classmethod",
                        "compile",
                        "complex",
                        "delattr",
                        "dict",
                        "dir",
                        "divmod",
                        "enumerate",
                        "eval",
                        "filter",
                        "float",
                        "format",
                        "frozendict",
                        "frozenset",
                        "getattr",
                        "globals",
                        "hasattr",
                        "hash",
                        "hex",
                        "id",
                        "input",
                        "int",
                        "isinstance",
                        "issubclass",
                        "iter",
                        "len",
                        "list",
                        "locals",
                        "map",
                        "max",
                        "memoryview",
                        "min",
                        "next",
                        "object",
                        "oct",
                        "open",
                        "ord",
                        "pow",
                        "print",
                        "property",
                        "range",
                        "repr",
                        "reversed",
                        "round",
                        "sentinel",
                        "set",
                        "setattr",
                        "slice",
                        "sorted",
                        "staticmethod",
                        "str",
                        "sum",
                        "super",
                        "tuple",
                        "type",
                        "vars",
                        "zip",
                    ],
                    r"(?<!\.)",
                    r"\b",
                    T::NameBuiltin,
                ),
                rule(
                    r"(?<!\.)(self|Ellipsis|NotImplemented|cls)\b",
                    T::NameBuiltinPseudo,
                ),
                words_rule(
                    &[
                        "ArithmeticError",
                        "AssertionError",
                        "AttributeError",
                        "BaseException",
                        "BufferError",
                        "BytesWarning",
                        "DeprecationWarning",
                        "EOFError",
                        "EnvironmentError",
                        "Exception",
                        "FloatingPointError",
                        "FutureWarning",
                        "GeneratorExit",
                        "IOError",
                        "ImportError",
                        "ImportWarning",
                        "IndentationError",
                        "IndexError",
                        "KeyError",
                        "KeyboardInterrupt",
                        "LookupError",
                        "MemoryError",
                        "NameError",
                        "NotImplementedError",
                        "OSError",
                        "OverflowError",
                        "PendingDeprecationWarning",
                        "ReferenceError",
                        "ResourceWarning",
                        "RuntimeError",
                        "RuntimeWarning",
                        "StopIteration",
                        "SyntaxError",
                        "SyntaxWarning",
                        "SystemError",
                        "SystemExit",
                        "TabError",
                        "TypeError",
                        "UnboundLocalError",
                        "UnicodeDecodeError",
                        "UnicodeEncodeError",
                        "UnicodeError",
                        "UnicodeTranslateError",
                        "UnicodeWarning",
                        "UserWarning",
                        "ValueError",
                        "VMSError",
                        "Warning",
                        "WindowsError",
                        "ZeroDivisionError",
                        // new builtin exceptions from PEP 3151
                        "BlockingIOError",
                        "ChildProcessError",
                        "ConnectionError",
                        "BrokenPipeError",
                        "ConnectionAbortedError",
                        "ConnectionRefusedError",
                        "ConnectionResetError",
                        "FileExistsError",
                        "FileNotFoundError",
                        "InterruptedError",
                        "IsADirectoryError",
                        "NotADirectoryError",
                        "PermissionError",
                        "ProcessLookupError",
                        "TimeoutError",
                        // others new in Python 3
                        "StopAsyncIteration",
                        "ModuleNotFoundError",
                        "RecursionError",
                        "EncodingWarning",
                    ],
                    r"(?<!\.)",
                    r"\b",
                    T::NameException,
                ),
            ],
        ),
        (
            "magicfuncs",
            vec![words_rule(
                &[
                    "__abs__",
                    "__add__",
                    "__aenter__",
                    "__aexit__",
                    "__aiter__",
                    "__and__",
                    "__anext__",
                    "__await__",
                    "__bool__",
                    "__bytes__",
                    "__call__",
                    "__complex__",
                    "__contains__",
                    "__del__",
                    "__delattr__",
                    "__delete__",
                    "__delitem__",
                    "__dir__",
                    "__divmod__",
                    "__enter__",
                    "__eq__",
                    "__exit__",
                    "__float__",
                    "__floordiv__",
                    "__format__",
                    "__ge__",
                    "__get__",
                    "__getattr__",
                    "__getattribute__",
                    "__getitem__",
                    "__gt__",
                    "__hash__",
                    "__iadd__",
                    "__iand__",
                    "__ifloordiv__",
                    "__ilshift__",
                    "__imatmul__",
                    "__imod__",
                    "__imul__",
                    "__index__",
                    "__init__",
                    "__instancecheck__",
                    "__int__",
                    "__invert__",
                    "__ior__",
                    "__ipow__",
                    "__irshift__",
                    "__isub__",
                    "__iter__",
                    "__itruediv__",
                    "__ixor__",
                    "__le__",
                    "__len__",
                    "__length_hint__",
                    "__lshift__",
                    "__lt__",
                    "__matmul__",
                    "__missing__",
                    "__mod__",
                    "__mul__",
                    "__ne__",
                    "__neg__",
                    "__new__",
                    "__next__",
                    "__or__",
                    "__pos__",
                    "__pow__",
                    "__prepare__",
                    "__radd__",
                    "__rand__",
                    "__rdivmod__",
                    "__repr__",
                    "__reversed__",
                    "__rfloordiv__",
                    "__rlshift__",
                    "__rmatmul__",
                    "__rmod__",
                    "__rmul__",
                    "__ror__",
                    "__round__",
                    "__rpow__",
                    "__rrshift__",
                    "__rshift__",
                    "__rsub__",
                    "__rtruediv__",
                    "__rxor__",
                    "__set__",
                    "__setattr__",
                    "__setitem__",
                    "__str__",
                    "__sub__",
                    "__subclasscheck__",
                    "__truediv__",
                    "__xor__",
                ],
                "",
                r"\b",
                T::NameFunctionMagic,
            )],
        ),
        (
            "magicvars",
            vec![words_rule(
                &[
                    "__annotations__",
                    "__bases__",
                    "__class__",
                    "__closure__",
                    "__code__",
                    "__defaults__",
                    "__dict__",
                    "__doc__",
                    "__file__",
                    "__func__",
                    "__globals__",
                    "__kwdefaults__",
                    "__module__",
                    "__mro__",
                    "__name__",
                    "__objclass__",
                    "__qualname__",
                    "__self__",
                    "__slots__",
                    "__weakref__",
                ],
                "",
                r"\b",
                T::NameVariableMagic,
            )],
        ),
        (
            "numbers",
            vec![
                rule(
                    r"(\d(?:_?\d)*\.(?:\d(?:_?\d)*)?|(?:\d(?:_?\d)*)?\.\d(?:_?\d)*)([eE][+-]?\d(?:_?\d)*)?",
                    T::NumberFloat,
                ),
                rule(r"\d(?:_?\d)*[eE][+-]?\d(?:_?\d)*j?", T::NumberFloat),
                rule(r"0[oO](?:_?[0-7])+", T::NumberOct),
                rule(r"0[bB](?:_?[01])+", T::NumberBin),
                rule(r"0[xX](?:_?[a-fA-F0-9])+", T::NumberHex),
                rule(r"\d(?:_?\d)*", T::NumberInteger),
            ],
        ),
        (
            "name",
            vec![
                rule(&format!("@{uni_name}"), T::NameDecorator),
                // new matrix multiplication operator
                rule("@", T::Operator),
                rule(&uni_name, T::Name),
            ],
        ),
        (
            "funcname",
            vec![
                Include("magicfuncs"),
                rule_to(&uni_name, T::NameFunction, NewState::Pop(1)),
                default(NewState::Pop(1)),
            ],
        ),
        (
            "classname",
            vec![rule_to(&uni_name, T::NameClass, NewState::Pop(1))],
        ),
        (
            "import",
            vec![
                by_groups(
                    r"(\s+)(as)(\s+)",
                    vec![
                        token(T::Whitespace),
                        token(T::Keyword),
                        token(T::Whitespace),
                    ],
                ),
                rule(r"\.", T::NameNamespace),
                rule(&uni_name, T::NameNamespace),
                by_groups(
                    r"(\s*)(,)(\s*)",
                    vec![
                        token(T::Whitespace),
                        token(T::Operator),
                        token(T::Whitespace),
                    ],
                ),
                // all else: go back
                default(NewState::Pop(1)),
            ],
        ),
        (
            "fromimport",
            vec![
                by_groups_to(
                    r"(\s+)(import)\b",
                    ws(T::Whitespace, T::KeywordNamespace),
                    NewState::Pop(1),
                ),
                rule(r"\.", T::NameNamespace),
                // if None occurs here, it's "raise x from None", since None can never be a
                // module name
                rule_to(r"None\b", T::KeywordConstant, NewState::Pop(1)),
                rule(&uni_name, T::NameNamespace),
                default(NewState::Pop(1)),
            ],
        ),
        (
            "rfstringescape",
            vec![
                rule(r"\{\{", T::StringEscape),
                rule(r"\}\}", T::StringEscape),
            ],
        ),
        (
            "fstringescape",
            vec![Include("rfstringescape"), Include("stringescape")],
        ),
        (
            "bytesescape",
            vec![rule(
                r#"\\([\\abfnrtv"']|\n|x[a-fA-F0-9]{2}|[0-7]{1,3})"#,
                T::StringEscape,
            )],
        ),
        (
            "stringescape",
            vec![
                rule(
                    r"\\(N\{.*?\}|u[a-fA-F0-9]{4}|U[a-fA-F0-9]{8})",
                    T::StringEscape,
                ),
                Include("bytesescape"),
            ],
        ),
        ("fstrings-single", f_string_rules(T::StringSingle)),
        ("fstrings-double", f_string_rules(T::StringDouble)),
        ("strings-single", inner_string_rules(T::StringSingle)),
        ("strings-double", inner_string_rules(T::StringDouble)),
        (
            "dqf",
            vec![
                rule_to("\"", T::StringDouble, NewState::Pop(1)),
                // included here for raw strings
                rule(r#"\\\\|\\"|\\\n"#, T::StringEscape),
                Include("fstrings-double"),
            ],
        ),
        (
            "sqf",
            vec![
                rule_to("'", T::StringSingle, NewState::Pop(1)),
                rule(r"\\\\|\\'|\\\n", T::StringEscape),
                Include("fstrings-single"),
            ],
        ),
        (
            "dqs",
            vec![
                rule_to("\"", T::StringDouble, NewState::Pop(1)),
                rule(r#"\\\\|\\"|\\\n"#, T::StringEscape),
                Include("strings-double"),
            ],
        ),
        (
            "sqs",
            vec![
                rule_to("'", T::StringSingle, NewState::Pop(1)),
                rule(r"\\\\|\\'|\\\n", T::StringEscape),
                Include("strings-single"),
            ],
        ),
        (
            "tdqf",
            vec![
                rule_to("\"\"\"", T::StringDouble, NewState::Pop(1)),
                Include("fstrings-double"),
                rule(r"\n", T::StringDouble),
            ],
        ),
        (
            "tsqf",
            vec![
                rule_to("'''", T::StringSingle, NewState::Pop(1)),
                Include("fstrings-single"),
                rule(r"\n", T::StringSingle),
            ],
        ),
        (
            "tdqs",
            vec![
                rule_to("\"\"\"", T::StringDouble, NewState::Pop(1)),
                Include("strings-double"),
                rule(r"\n", T::StringDouble),
            ],
        ),
        (
            "tsqs",
            vec![
                rule_to("'''", T::StringSingle, NewState::Pop(1)),
                Include("strings-single"),
                rule(r"\n", T::StringSingle),
            ],
        ),
    ];
    LexerDef {
        states,
        flags: Flags {
            multiline: true,
            ..Flags::default()
        },
    }
}
