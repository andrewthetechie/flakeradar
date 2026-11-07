"""Generates the encoding label table and its fixture from CPython's codecs.

The reference reads a declared encoding through the interpreter's codecs, so the
labels it knows and the byte-to-character tables it decodes with come from
CPython rather than from WHATWG's registry. This script writes both:

  crates/flakeradar-core/src/encodings.rs              the production table
  crates/flakeradar-core/tests/fixtures/encodings.tsv  one row per label and byte

Run it from the repository root with the reference's interpreter, which is the
one whose codecs the reference used:

    /tmp/pyref/bin/python \
        docs/plans/rust-rewrite/review-135b/repro/spec/genencodings.py

The output is sorted and carries no timestamp, so running it twice gives the
same bytes.

The classification is measured rather than assumed:

- A label is a *text* encoding when `codecs.lookup(label)._is_text_encoding` is
  true. Everything else (`base64`, `hex`, `quopri`, `rot13`, `zlib`, `bz2`,
  `uu`, `idna` …) is left out of the table, so it stays an unknown label and
  answers 422, which is what the reference answers.
- A codec is *multi-byte* when one of its bytes is the start of a longer
  sequence, which shows up as decoding that byte alone raising a
  "truncated"/"incomplete" error. The reference refuses every one of these with
  `multi-byte encodings are not supported`, an ASCII-only body included, so they
  are refused here too.
- `utf-16` and its two unhyphenated spellings `utf-16le`/`utf-16be` are expat's
  own built-in names, so the label decides more than the codec does. The
  hyphenated spellings `utf-16-le`/`utf-16-be` are not built in, and the
  reference refuses them.
- An EBCDIC codec is single-byte but maps `A` to something other than `A`, and a
  stateful codec (`iso-2022-*`, `hz`, `utf-7`) changes meaning with escapes. The
  owner's decision (`00-fixes-2-context.md`) keeps 422 for both, so neither gets
  a table.

What a single-byte codec does with a byte it does not define is measured against
the reference rather than guessed: `windows-1252` with `0x81` and `0x80` in one
name gives two U+FFFD, so an undefined byte makes the whole document fall back to
the lossy UTF-8 read. The fixture records such a byte as `undef`.
"""

from __future__ import annotations

import codecs
import encodings
import encodings.aliases
import pkgutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[6]
TABLE_OUT = REPO / "crates/flakeradar-core/src/encodings.rs"
FIXTURE_OUT = REPO / "crates/flakeradar-core/tests/fixtures/encodings.tsv"

# The characters XML allows in an encoding name: a letter, then letters, digits,
# `.`, `_` and `-`. A label outside this set is one the reference reports as a
# declaration that is not well formed, whatever codec it names.
XML_NAME_CHARS = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-")


def is_xml_encoding_name(label: str) -> bool:
    """Whether `label` is a name an XML declaration may carry."""
    return bool(label) and label[0].isascii() and label[0].isalpha() and set(label) <= XML_NAME_CHARS


# The bytes the fixture covers. Every ASCII byte decodes to itself in every codec
# that gets a table, so the fixture holds one ASCII row per label and one row per
# high byte, where the codecs disagree.
HIGH_BYTES = range(0x80, 0x100)

# The codecs expat knows as multi-byte whatever their bytes look like, so the
# reference refuses them for any body.
MULTI_BYTE_CODECS = {"utf-16-le", "utf-16-be", "utf-32", "utf-32-le", "utf-32-be"}

# The codecs whose meaning changes with escape sequences or with the code page.
STATE_OR_EBCDIC = {"hz", "utf-7"}


def label_list() -> dict[str, str]:
    """Every label CPython resolves to a text codec, with that codec's name."""
    labels = set(encodings.aliases.aliases)
    labels |= set(encodings.aliases.aliases.values())
    for module in pkgutil.iter_modules(encodings.__path__):
        try:
            loaded = __import__("encodings." + module.name, fromlist=["*"])
        except Exception:  # noqa: BLE001 - a platform codec that will not import
            continue
        if hasattr(loaded, "getregentry"):
            labels.add(module.name)
    resolved: dict[str, str] = {}
    for label in sorted(labels):
        try:
            info = codecs.lookup(label)
        except LookupError:
            continue
        if not info._is_text_encoding:  # noqa: SLF001 - the only shape this flag has
            continue
        resolved[label] = info.name
    return resolved


def normalize(label: str) -> str:
    """Python's label normalization: lowercase, and runs of non-alphanumerics to `_`."""
    out = []
    underscore = False
    for char in label.lower():
        if char.isalnum():
            if underscore and out:
                out.append("_")
            underscore = False
            out.append(char)
        else:
            underscore = True
    return "".join(out)


def single_byte_error(codec: str, byte: int) -> str:
    """How `codec` refuses the byte on its own, or `""` when it accepts it."""
    try:
        bytes([byte]).decode(codec)
    except UnicodeDecodeError as error:
        return error.reason
    except Exception:  # noqa: BLE001 - a codec that raises anything else refuses the byte too
        return "other"
    return ""


def is_multi_byte(codec: str) -> bool:
    """Whether a byte of this codec can start a longer sequence."""
    return any(
        single_byte_error(codec, byte).startswith(
            ("incomplete", "unexpected end", "truncated")
        )
        for byte in HIGH_BYTES
    )


def refuses_replace(codec: str) -> bool:
    """Whether the codec refuses the error handler expat's own table uses.

    pyexpat builds its table by decoding each high byte with `errors="replace"`,
    so a codec that does not implement that handler — `idna`, `punycode`,
    `undefined` — is one the reference cannot read at all, and it answers 422.
    """
    for byte in HIGH_BYTES:
        try:
            bytes([byte]).decode(codec, "replace")
        except Exception:  # noqa: BLE001 - the codec refused the handler or the byte
            return True
    return False


def is_ebcdic(codec: str) -> bool:
    """Whether the codec maps ASCII `A` to something other than `A`."""
    try:
        return bytes([0x41]).decode(codec) != "A"
    except Exception:  # noqa: BLE001 - a codec that cannot decode `A` is unusable anyway
        return True


def is_stateful(codec: str) -> bool:
    """Whether the codec's decoding depends on escape sequences."""
    return codec.startswith("iso2022") or codec in STATE_OR_EBCDIC


def maps_ascii_identically(codec: str) -> bool:
    """Whether every ASCII byte decodes to the character of the same code."""
    try:
        return bytes(range(0x80)).decode(codec) == "".join(chr(b) for b in range(0x80))
    except Exception:  # noqa: BLE001 - a codec that cannot decode ASCII is not one expat uses
        return False


def classify(codec: str) -> tuple[str, str]:
    """The rule a canonical codec name gets, and the code page a table would name."""
    if codec in {"utf-8", "utf-8-sig"}:
        return "Utf8", ""
    if codec in MULTI_BYTE_CODECS or codec == "utf-16" or is_multi_byte(codec):
        return "Refused", ""
    if is_stateful(codec) or is_ebcdic(codec) or refuses_replace(codec):
        return "Refused", ""
    if not maps_ascii_identically(codec):
        # `cp864` is single-byte but rewrites ASCII `%`, so the reference cannot
        # build a table from it either: it reads the whole document lossily,
        # which is what `Lossy` does.
        return "Lossy", ""
    return "Single", codec


def table(codec: str) -> list[str | None]:
    """The character each byte decodes to, or `None` where the code page defines none."""
    entries: list[str | None] = []
    for byte in range(0x100):
        try:
            text = bytes([byte]).decode(codec)
        except Exception:  # noqa: BLE001 - an undefined byte
            entries.append(None)
            continue
        entries.append(text if len(text) == 1 else None)
    return entries


def rust_string(text: str) -> str:
    """A Rust string literal holding `text`, escaping what has to be escaped."""
    out = []
    for char in text:
        point = ord(char)
        if char == '"':
            out.append('\\"')
        elif char == "\\":
            out.append("\\\\")
        elif 0x20 <= point < 0x7F:
            out.append(char)
        else:
            out.append(f"\\u{{{point:x}}}")
    return '"' + "".join(out) + '"'


def rust_array(values: list[int]) -> str:
    """A Rust byte-array literal."""
    return "[" + ", ".join(str(value) for value in values) + "]"


def main() -> int:
    labels = label_list()
    rules: dict[str, tuple[str, str]] = {}
    tables: dict[str, list[str | None]] = {}
    for label, codec in labels.items():
        key = normalize(label)
        kind, name = classify(codec)
        rules[key] = (kind, name)
        if kind == "Single" and name not in tables:
            tables[name] = table(name)

    table_names = sorted(tables)
    source = [
        "//! The encoding labels this server reads, and the table each decodes with.",
        "//!",
        "//! Generated from `CPython`'s codecs by",
        "//! `docs/plans/rust-rewrite/review-135b/repro/spec/genencodings.py`. Do not",
        "//! edit by hand: change the generator and run it again, which gives the same",
        "//! bytes as long as the interpreter's codecs do not change.",
        "",
        "// Rust guideline compliant 2026-07-21",
        "",
        "use std::collections::HashMap;",
        "use std::sync::LazyLock;",
        "",
        "/// How one encoding label decodes a document.",
        "#[derive(Debug, Clone, Copy)]",
        "pub(crate) enum Rule {",
        "    /// UTF-8, including the labels that only strip its byte-order mark.",
        "    Utf8,",
        "    /// UTF-16, and which endianness the label itself names.",
        "    Utf16(Utf16Order),",
        "    /// A single-byte code page, decoded through its own table.",
        "    Single(SingleByte),",
        "    /// A codec the reference cannot read either, so the document falls back",
        "    /// to the lossy UTF-8 read.",
        "    Lossy,",
        "    /// A codec this server refuses, as the reference refuses its family.",
        "    Refused,",
        "}",
        "",
        "/// The endianness a UTF-16 label names.",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq)]",
        "pub(crate) enum Utf16Order {",
        "    /// `utf-16`: a byte-order mark decides, and so does a leading `<`.",
        "    Any,",
        "    /// `utf-16le`: the bytes are little-endian.",
        "    Little,",
        "    /// `utf-16be`: the bytes are big-endian.",
        "    Big,",
        "}",
        "",
        "/// One single-byte code page's decoding.",
        "#[derive(Debug, Clone, Copy)]",
        "pub(crate) struct SingleByte {",
        "    /// The character each byte decodes to, in byte order; an undefined byte",
        "    /// holds U+FFFD here and is never read.",
        "    pub(crate) table: &'static str,",
        "    /// The bytes this code page does not define, one `char` per byte's code.",
        "    pub(crate) undefined: &'static str,",
        "}",
        "",
        "impl SingleByte {",
        "    /// The text `bytes` decode to, or `None` when one of them is undefined.",
        "    ///",
        "    /// An undefined byte makes the whole document fall back to the lossy UTF-8",
        "    /// read, which is what the reference does with such a document.",
        "    pub(crate) fn decode(&self, bytes: &[u8]) -> Option<String> {",
        "        let characters: Vec<char> = self.table.chars().collect();",
        "        let undefined: Vec<u32> = self.undefined.chars().map(u32::from).collect();",
        "        let mut text = String::with_capacity(bytes.len());",
        "        for byte in bytes {",
        "            if undefined.contains(&u32::from(*byte)) {",
        "                return None;",
        "            }",
        "            text.push(*characters.get(usize::from(*byte))?);",
        "        }",
        "        Some(text)",
        "    }",
        "}",
        "",
        "/// Every single-byte code page, by the codec name the rule table points at.",
        "static TABLES: &[(&str, SingleByte)] = &[",
    ]
    for name in table_names:
        chars = []
        undefined = []
        for byte, char in enumerate(tables[name]):
            if char is None:
                undefined.append(byte)
                chars.append("\uFFFD")
            else:
                chars.append(char)
        source += [
            "    (",
            f"        {rust_string(name)},",
            "        SingleByte {",
            f"            table: {rust_string(''.join(chars))},",
            f"            undefined: {rust_string(''.join(chr(byte) for byte in undefined))},",
            "        },",
            "    ),",
        ]
    source += [
        "];",
        "",
        "/// The rule each label decodes by, keyed by Python's normalized spelling.",
        "static RULES: LazyLock<HashMap<&'static str, Rule>> = LazyLock::new(|| {",
        "    let mut rules = HashMap::new();",
    ]
    for key in sorted(rules):
        kind, name = rules[key]
        if kind == "Utf8":
            value = "Rule::Utf8"
        elif kind == "Lossy":
            value = "Rule::Lossy"
        elif kind == "utf16":
            value = f"Rule::Utf16(Utf16Order::{name})"
        elif kind == "Single":
            value = f"Rule::Single(table({rust_string(name)}))"
        else:
            value = "Rule::Refused"
        source.append(f"    rules.insert({rust_string(key)}, {value});")
    source += [
        "    rules",
        "});",
        "",
        "/// The rule `label` decodes by, when `CPython`'s codecs know that label.",
        "pub(crate) fn rule_for(label: &str) -> Option<Rule> {",
        "    RULES.get(normalized(label).as_str()).copied()",
        "}",
        "",
        "/// The code page `name` holds, which the rule table has already named.",
        "#[expect(",
        "    clippy::panic,",
        "    reason = \"every code page the rule table names is in this file, so a missing \\",
        "              one is a programming error that must stop the process\"",
        ")]",
        "fn table(name: &str) -> SingleByte {",
        "    TABLES",
        "        .iter()",
        "        .find_map(|(codec, table)| (*codec == name).then_some(*table))",
        "        .unwrap_or_else(|| panic!(\"the {name} code page has no table\"))",
        "}",
        "",
        "/// Python's label normalization: lowercase, and every run of non-alphanumeric",
        "/// characters becomes one `_`.",
        "fn normalized(label: &str) -> String {",
        "    let mut out = String::with_capacity(label.len());",
        "    let mut underscore = false;",
        "    for char in label.chars() {",
        "        if char.is_alphanumeric() {",
        "            if underscore && !out.is_empty() {",
        "                out.push('_');",
        "            }",
        "            underscore = false;",
        "            for lowered in char.to_lowercase() {",
        "                out.push(lowered);",
        "            }",
        "        } else {",
        "            underscore = true;",
        "        }",
        "    }",
        "    out",
        "}",
    ]
    TABLE_OUT.write_text("\n".join(source) + "\n", encoding="utf-8")
    # The table is Rust source, so it is formatted by the same rustfmt the gate
    # runs; without this, regenerating would leave a `cargo fmt --check` diff.
    formatted = subprocess.run(
        ["rustfmt", "--edition", "2024", str(TABLE_OUT)],
        check=False,
        capture_output=True,
    )
    if formatted.returncode != 0:
        print(
            "rustfmt is not on PATH, so the table is left unformatted: "
            + formatted.stderr.decode(errors="replace"),
            file=sys.stderr,
        )

    fixture = [
        "# One row per encoding label and byte: the answer the reference gives for a",
        "# JUnit document that declares that label and puts that byte in a case's name.",
        "# Generated from CPython's codecs by genencodings.py; do not edit by hand.",
        "#",
        "# label <TAB> byte (hex, or `ascii` for the printable-ASCII probe) <TAB> answer",
        "# answer: the code point the case's name holds in hex, or `refuse` for a label",
        "# the reference refuses. A byte the code page leaves undefined, and a character",
        "# the probe document cannot carry unescaped, put the whole document back through",
        "# the lossy UTF-8 read, so their answer is U+FFFD.",
    ]

    def answer_for(entries: list[str | None] | None, byte: int, refuse: bool) -> str:
        """The code point a probe of `byte` leaves in the case's name."""
        if refuse:
            return "refuse"
        char = entries[byte] if entries is not None else None
        if char is None or char in '"&<':
            # No table, or a character an attribute value cannot carry (`>` can,
            # `<`, `&` and the quoting `"` cannot): the
            # document is re-read lossily, so the byte reads as what lossy UTF-8
            # makes of it.
            lossy = bytes([byte]).decode("utf-8", "replace")
            return f"{ord(lossy):x}"
        return f"{ord(char):x}"

    for key in sorted(rules):
        kind, name = rules[key]
        entries = tables.get(name) if kind == "Single" else None
        refuse = kind == "Refused" or not is_xml_encoding_name(key)
        for byte in HIGH_BYTES:
            fixture.append(f"{key}\t{byte:02x}\t{answer_for(entries, byte, refuse)}")
        fixture.append(f"{key}\tascii\t{answer_for(entries, 0x41, refuse)}")
    FIXTURE_OUT.write_text("\n".join(fixture) + "\n", encoding="utf-8")

    print(f"{len(rules)} labels, {len(table_names)} code pages", file=sys.stderr)
    print(f"wrote {TABLE_OUT}", file=sys.stderr)
    print(f"wrote {FIXTURE_OUT}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
