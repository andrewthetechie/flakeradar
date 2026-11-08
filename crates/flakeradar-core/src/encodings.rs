//! The encoding labels this server reads, and the table each decodes with.
//!
//! Derived from `CPython`'s codecs. Do not edit by hand.

// Rust guideline compliant 2026-07-21

use std::collections::HashMap;
use std::sync::LazyLock;

/// How one encoding label decodes a document.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Rule {
    /// UTF-8, including the labels that only strip its byte-order mark.
    Utf8,
    /// UTF-16, and which endianness the label itself names.
    Utf16(Utf16Order),
    /// A single-byte code page, decoded through its own table.
    Single(SingleByte),
    /// A single-byte codec with no table: one that rewrites ASCII, so no
    /// document is read through it and the lossy UTF-8 read takes the text.
    Lossy,
    /// A multi-byte, stateful or EBCDIC codec, which is refused outright.
    Refused,
}

/// The endianness a UTF-16 label names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Utf16Order {
    /// `utf-16`: a byte-order mark decides, and so does a leading `<`.
    Any,
    /// `utf-16le`: the bytes are little-endian.
    Little,
    /// `utf-16be`: the bytes are big-endian.
    Big,
}

/// One single-byte code page's decoding.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SingleByte {
    /// The character each byte decodes to, in byte order; an undefined byte
    /// holds U+FFFD here and is never read.
    pub(crate) table: &'static str,
    /// The bytes this code page does not define, one `char` per byte's code.
    pub(crate) undefined: &'static str,
}

impl SingleByte {
    /// The text `bytes` decode to, or `None` when one of them is undefined.
    ///
    /// An undefined byte makes the whole document fall back to the lossy
    /// UTF-8 read: a code page with no character for the byte has no answer
    /// for the document.
    pub(crate) fn decode(&self, bytes: &[u8]) -> Option<String> {
        let characters: Vec<char> = self.table.chars().collect();
        let undefined: Vec<u32> = self.undefined.chars().map(u32::from).collect();
        let mut text = String::with_capacity(bytes.len());
        for byte in bytes {
            if undefined.contains(&u32::from(*byte)) {
                return None;
            }
            text.push(*characters.get(usize::from(*byte))?);
        }
        Some(text)
    }
}

/// Every single-byte code page, by the codec name the rule table points at.
static TABLES: &[(&str, SingleByte)] = &[
    (
        "ascii",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}",
            undefined: "\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{a1}\u{a2}\u{a3}\u{a4}\u{a5}\u{a6}\u{a7}\u{a8}\u{a9}\u{aa}\u{ab}\u{ac}\u{ad}\u{ae}\u{af}\u{b0}\u{b1}\u{b2}\u{b3}\u{b4}\u{b5}\u{b6}\u{b7}\u{b8}\u{b9}\u{ba}\u{bb}\u{bc}\u{bd}\u{be}\u{bf}\u{c0}\u{c1}\u{c2}\u{c3}\u{c4}\u{c5}\u{c6}\u{c7}\u{c8}\u{c9}\u{ca}\u{cb}\u{cc}\u{cd}\u{ce}\u{cf}\u{d0}\u{d1}\u{d2}\u{d3}\u{d4}\u{d5}\u{d6}\u{d7}\u{d8}\u{d9}\u{da}\u{db}\u{dc}\u{dd}\u{de}\u{df}\u{e0}\u{e1}\u{e2}\u{e3}\u{e4}\u{e5}\u{e6}\u{e7}\u{e8}\u{e9}\u{ea}\u{eb}\u{ec}\u{ed}\u{ee}\u{ef}\u{f0}\u{f1}\u{f2}\u{f3}\u{f4}\u{f5}\u{f6}\u{f7}\u{f8}\u{f9}\u{fa}\u{fb}\u{fc}\u{fd}\u{fe}\u{ff}",
        },
    ),
    (
        "charmap",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{a1}\u{a2}\u{a3}\u{a4}\u{a5}\u{a6}\u{a7}\u{a8}\u{a9}\u{aa}\u{ab}\u{ac}\u{ad}\u{ae}\u{af}\u{b0}\u{b1}\u{b2}\u{b3}\u{b4}\u{b5}\u{b6}\u{b7}\u{b8}\u{b9}\u{ba}\u{bb}\u{bc}\u{bd}\u{be}\u{bf}\u{c0}\u{c1}\u{c2}\u{c3}\u{c4}\u{c5}\u{c6}\u{c7}\u{c8}\u{c9}\u{ca}\u{cb}\u{cc}\u{cd}\u{ce}\u{cf}\u{d0}\u{d1}\u{d2}\u{d3}\u{d4}\u{d5}\u{d6}\u{d7}\u{d8}\u{d9}\u{da}\u{db}\u{dc}\u{dd}\u{de}\u{df}\u{e0}\u{e1}\u{e2}\u{e3}\u{e4}\u{e5}\u{e6}\u{e7}\u{e8}\u{e9}\u{ea}\u{eb}\u{ec}\u{ed}\u{ee}\u{ef}\u{f0}\u{f1}\u{f2}\u{f3}\u{f4}\u{f5}\u{f6}\u{f7}\u{f8}\u{f9}\u{fa}\u{fb}\u{fc}\u{fd}\u{fe}\u{ff}",
            undefined: "",
        },
    ),
    (
        "cp1006",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{6f0}\u{6f1}\u{6f2}\u{6f3}\u{6f4}\u{6f5}\u{6f6}\u{6f7}\u{6f8}\u{6f9}\u{60c}\u{61b}\u{ad}\u{61f}\u{fe81}\u{fe8d}\u{fe8e}\u{fe8e}\u{fe8f}\u{fe91}\u{fb56}\u{fb58}\u{fe93}\u{fe95}\u{fe97}\u{fb66}\u{fb68}\u{fe99}\u{fe9b}\u{fe9d}\u{fe9f}\u{fb7a}\u{fb7c}\u{fea1}\u{fea3}\u{fea5}\u{fea7}\u{fea9}\u{fb84}\u{feab}\u{fead}\u{fb8c}\u{feaf}\u{fb8a}\u{feb1}\u{feb3}\u{feb5}\u{feb7}\u{feb9}\u{febb}\u{febd}\u{febf}\u{fec1}\u{fec5}\u{fec9}\u{feca}\u{fecb}\u{fecc}\u{fecd}\u{fece}\u{fecf}\u{fed0}\u{fed1}\u{fed3}\u{fed5}\u{fed7}\u{fed9}\u{fedb}\u{fb92}\u{fb94}\u{fedd}\u{fedf}\u{fee0}\u{fee1}\u{fee3}\u{fb9e}\u{fee5}\u{fee7}\u{fe85}\u{feed}\u{fba6}\u{fba8}\u{fba9}\u{fbaa}\u{fe80}\u{fe89}\u{fe8a}\u{fe8b}\u{fef1}\u{fef2}\u{fef3}\u{fbb0}\u{fbae}\u{fe7c}\u{fe7d}",
            undefined: "",
        },
    ),
    (
        "cp1125",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{410}\u{411}\u{412}\u{413}\u{414}\u{415}\u{416}\u{417}\u{418}\u{419}\u{41a}\u{41b}\u{41c}\u{41d}\u{41e}\u{41f}\u{420}\u{421}\u{422}\u{423}\u{424}\u{425}\u{426}\u{427}\u{428}\u{429}\u{42a}\u{42b}\u{42c}\u{42d}\u{42e}\u{42f}\u{430}\u{431}\u{432}\u{433}\u{434}\u{435}\u{436}\u{437}\u{438}\u{439}\u{43a}\u{43b}\u{43c}\u{43d}\u{43e}\u{43f}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{2561}\u{2562}\u{2556}\u{2555}\u{2563}\u{2551}\u{2557}\u{255d}\u{255c}\u{255b}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{255e}\u{255f}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{2567}\u{2568}\u{2564}\u{2565}\u{2559}\u{2558}\u{2552}\u{2553}\u{256b}\u{256a}\u{2518}\u{250c}\u{2588}\u{2584}\u{258c}\u{2590}\u{2580}\u{440}\u{441}\u{442}\u{443}\u{444}\u{445}\u{446}\u{447}\u{448}\u{449}\u{44a}\u{44b}\u{44c}\u{44d}\u{44e}\u{44f}\u{401}\u{451}\u{490}\u{491}\u{404}\u{454}\u{406}\u{456}\u{407}\u{457}\u{b7}\u{221a}\u{2116}\u{a4}\u{25a0}\u{a0}",
            undefined: "",
        },
    ),
    (
        "cp1250",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{20ac}\u{fffd}\u{201a}\u{fffd}\u{201e}\u{2026}\u{2020}\u{2021}\u{fffd}\u{2030}\u{160}\u{2039}\u{15a}\u{164}\u{17d}\u{179}\u{fffd}\u{2018}\u{2019}\u{201c}\u{201d}\u{2022}\u{2013}\u{2014}\u{fffd}\u{2122}\u{161}\u{203a}\u{15b}\u{165}\u{17e}\u{17a}\u{a0}\u{2c7}\u{2d8}\u{141}\u{a4}\u{104}\u{a6}\u{a7}\u{a8}\u{a9}\u{15e}\u{ab}\u{ac}\u{ad}\u{ae}\u{17b}\u{b0}\u{b1}\u{2db}\u{142}\u{b4}\u{b5}\u{b6}\u{b7}\u{b8}\u{105}\u{15f}\u{bb}\u{13d}\u{2dd}\u{13e}\u{17c}\u{154}\u{c1}\u{c2}\u{102}\u{c4}\u{139}\u{106}\u{c7}\u{10c}\u{c9}\u{118}\u{cb}\u{11a}\u{cd}\u{ce}\u{10e}\u{110}\u{143}\u{147}\u{d3}\u{d4}\u{150}\u{d6}\u{d7}\u{158}\u{16e}\u{da}\u{170}\u{dc}\u{dd}\u{162}\u{df}\u{155}\u{e1}\u{e2}\u{103}\u{e4}\u{13a}\u{107}\u{e7}\u{10d}\u{e9}\u{119}\u{eb}\u{11b}\u{ed}\u{ee}\u{10f}\u{111}\u{144}\u{148}\u{f3}\u{f4}\u{151}\u{f6}\u{f7}\u{159}\u{16f}\u{fa}\u{171}\u{fc}\u{fd}\u{163}\u{2d9}",
            undefined: "\u{81}\u{83}\u{88}\u{90}\u{98}",
        },
    ),
    (
        "cp1251",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{402}\u{403}\u{201a}\u{453}\u{201e}\u{2026}\u{2020}\u{2021}\u{20ac}\u{2030}\u{409}\u{2039}\u{40a}\u{40c}\u{40b}\u{40f}\u{452}\u{2018}\u{2019}\u{201c}\u{201d}\u{2022}\u{2013}\u{2014}\u{fffd}\u{2122}\u{459}\u{203a}\u{45a}\u{45c}\u{45b}\u{45f}\u{a0}\u{40e}\u{45e}\u{408}\u{a4}\u{490}\u{a6}\u{a7}\u{401}\u{a9}\u{404}\u{ab}\u{ac}\u{ad}\u{ae}\u{407}\u{b0}\u{b1}\u{406}\u{456}\u{491}\u{b5}\u{b6}\u{b7}\u{451}\u{2116}\u{454}\u{bb}\u{458}\u{405}\u{455}\u{457}\u{410}\u{411}\u{412}\u{413}\u{414}\u{415}\u{416}\u{417}\u{418}\u{419}\u{41a}\u{41b}\u{41c}\u{41d}\u{41e}\u{41f}\u{420}\u{421}\u{422}\u{423}\u{424}\u{425}\u{426}\u{427}\u{428}\u{429}\u{42a}\u{42b}\u{42c}\u{42d}\u{42e}\u{42f}\u{430}\u{431}\u{432}\u{433}\u{434}\u{435}\u{436}\u{437}\u{438}\u{439}\u{43a}\u{43b}\u{43c}\u{43d}\u{43e}\u{43f}\u{440}\u{441}\u{442}\u{443}\u{444}\u{445}\u{446}\u{447}\u{448}\u{449}\u{44a}\u{44b}\u{44c}\u{44d}\u{44e}\u{44f}",
            undefined: "\u{98}",
        },
    ),
    (
        "cp1252",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{20ac}\u{fffd}\u{201a}\u{192}\u{201e}\u{2026}\u{2020}\u{2021}\u{2c6}\u{2030}\u{160}\u{2039}\u{152}\u{fffd}\u{17d}\u{fffd}\u{fffd}\u{2018}\u{2019}\u{201c}\u{201d}\u{2022}\u{2013}\u{2014}\u{2dc}\u{2122}\u{161}\u{203a}\u{153}\u{fffd}\u{17e}\u{178}\u{a0}\u{a1}\u{a2}\u{a3}\u{a4}\u{a5}\u{a6}\u{a7}\u{a8}\u{a9}\u{aa}\u{ab}\u{ac}\u{ad}\u{ae}\u{af}\u{b0}\u{b1}\u{b2}\u{b3}\u{b4}\u{b5}\u{b6}\u{b7}\u{b8}\u{b9}\u{ba}\u{bb}\u{bc}\u{bd}\u{be}\u{bf}\u{c0}\u{c1}\u{c2}\u{c3}\u{c4}\u{c5}\u{c6}\u{c7}\u{c8}\u{c9}\u{ca}\u{cb}\u{cc}\u{cd}\u{ce}\u{cf}\u{d0}\u{d1}\u{d2}\u{d3}\u{d4}\u{d5}\u{d6}\u{d7}\u{d8}\u{d9}\u{da}\u{db}\u{dc}\u{dd}\u{de}\u{df}\u{e0}\u{e1}\u{e2}\u{e3}\u{e4}\u{e5}\u{e6}\u{e7}\u{e8}\u{e9}\u{ea}\u{eb}\u{ec}\u{ed}\u{ee}\u{ef}\u{f0}\u{f1}\u{f2}\u{f3}\u{f4}\u{f5}\u{f6}\u{f7}\u{f8}\u{f9}\u{fa}\u{fb}\u{fc}\u{fd}\u{fe}\u{ff}",
            undefined: "\u{81}\u{8d}\u{8f}\u{90}\u{9d}",
        },
    ),
    (
        "cp1253",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{20ac}\u{fffd}\u{201a}\u{192}\u{201e}\u{2026}\u{2020}\u{2021}\u{fffd}\u{2030}\u{fffd}\u{2039}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{2018}\u{2019}\u{201c}\u{201d}\u{2022}\u{2013}\u{2014}\u{fffd}\u{2122}\u{fffd}\u{203a}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{a0}\u{385}\u{386}\u{a3}\u{a4}\u{a5}\u{a6}\u{a7}\u{a8}\u{a9}\u{fffd}\u{ab}\u{ac}\u{ad}\u{ae}\u{2015}\u{b0}\u{b1}\u{b2}\u{b3}\u{384}\u{b5}\u{b6}\u{b7}\u{388}\u{389}\u{38a}\u{bb}\u{38c}\u{bd}\u{38e}\u{38f}\u{390}\u{391}\u{392}\u{393}\u{394}\u{395}\u{396}\u{397}\u{398}\u{399}\u{39a}\u{39b}\u{39c}\u{39d}\u{39e}\u{39f}\u{3a0}\u{3a1}\u{fffd}\u{3a3}\u{3a4}\u{3a5}\u{3a6}\u{3a7}\u{3a8}\u{3a9}\u{3aa}\u{3ab}\u{3ac}\u{3ad}\u{3ae}\u{3af}\u{3b0}\u{3b1}\u{3b2}\u{3b3}\u{3b4}\u{3b5}\u{3b6}\u{3b7}\u{3b8}\u{3b9}\u{3ba}\u{3bb}\u{3bc}\u{3bd}\u{3be}\u{3bf}\u{3c0}\u{3c1}\u{3c2}\u{3c3}\u{3c4}\u{3c5}\u{3c6}\u{3c7}\u{3c8}\u{3c9}\u{3ca}\u{3cb}\u{3cc}\u{3cd}\u{3ce}\u{fffd}",
            undefined: "\u{81}\u{88}\u{8a}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{98}\u{9a}\u{9c}\u{9d}\u{9e}\u{9f}\u{aa}\u{d2}\u{ff}",
        },
    ),
    (
        "cp1254",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{20ac}\u{fffd}\u{201a}\u{192}\u{201e}\u{2026}\u{2020}\u{2021}\u{2c6}\u{2030}\u{160}\u{2039}\u{152}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{2018}\u{2019}\u{201c}\u{201d}\u{2022}\u{2013}\u{2014}\u{2dc}\u{2122}\u{161}\u{203a}\u{153}\u{fffd}\u{fffd}\u{178}\u{a0}\u{a1}\u{a2}\u{a3}\u{a4}\u{a5}\u{a6}\u{a7}\u{a8}\u{a9}\u{aa}\u{ab}\u{ac}\u{ad}\u{ae}\u{af}\u{b0}\u{b1}\u{b2}\u{b3}\u{b4}\u{b5}\u{b6}\u{b7}\u{b8}\u{b9}\u{ba}\u{bb}\u{bc}\u{bd}\u{be}\u{bf}\u{c0}\u{c1}\u{c2}\u{c3}\u{c4}\u{c5}\u{c6}\u{c7}\u{c8}\u{c9}\u{ca}\u{cb}\u{cc}\u{cd}\u{ce}\u{cf}\u{11e}\u{d1}\u{d2}\u{d3}\u{d4}\u{d5}\u{d6}\u{d7}\u{d8}\u{d9}\u{da}\u{db}\u{dc}\u{130}\u{15e}\u{df}\u{e0}\u{e1}\u{e2}\u{e3}\u{e4}\u{e5}\u{e6}\u{e7}\u{e8}\u{e9}\u{ea}\u{eb}\u{ec}\u{ed}\u{ee}\u{ef}\u{11f}\u{f1}\u{f2}\u{f3}\u{f4}\u{f5}\u{f6}\u{f7}\u{f8}\u{f9}\u{fa}\u{fb}\u{fc}\u{131}\u{15f}\u{ff}",
            undefined: "\u{81}\u{8d}\u{8e}\u{8f}\u{90}\u{9d}\u{9e}",
        },
    ),
    (
        "cp1255",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{20ac}\u{fffd}\u{201a}\u{192}\u{201e}\u{2026}\u{2020}\u{2021}\u{2c6}\u{2030}\u{fffd}\u{2039}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{2018}\u{2019}\u{201c}\u{201d}\u{2022}\u{2013}\u{2014}\u{2dc}\u{2122}\u{fffd}\u{203a}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{a0}\u{a1}\u{a2}\u{a3}\u{20aa}\u{a5}\u{a6}\u{a7}\u{a8}\u{a9}\u{d7}\u{ab}\u{ac}\u{ad}\u{ae}\u{af}\u{b0}\u{b1}\u{b2}\u{b3}\u{b4}\u{b5}\u{b6}\u{b7}\u{b8}\u{b9}\u{f7}\u{bb}\u{bc}\u{bd}\u{be}\u{bf}\u{5b0}\u{5b1}\u{5b2}\u{5b3}\u{5b4}\u{5b5}\u{5b6}\u{5b7}\u{5b8}\u{5b9}\u{fffd}\u{5bb}\u{5bc}\u{5bd}\u{5be}\u{5bf}\u{5c0}\u{5c1}\u{5c2}\u{5c3}\u{5f0}\u{5f1}\u{5f2}\u{5f3}\u{5f4}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{5d0}\u{5d1}\u{5d2}\u{5d3}\u{5d4}\u{5d5}\u{5d6}\u{5d7}\u{5d8}\u{5d9}\u{5da}\u{5db}\u{5dc}\u{5dd}\u{5de}\u{5df}\u{5e0}\u{5e1}\u{5e2}\u{5e3}\u{5e4}\u{5e5}\u{5e6}\u{5e7}\u{5e8}\u{5e9}\u{5ea}\u{fffd}\u{fffd}\u{200e}\u{200f}\u{fffd}",
            undefined: "\u{81}\u{8a}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{9a}\u{9c}\u{9d}\u{9e}\u{9f}\u{ca}\u{d9}\u{da}\u{db}\u{dc}\u{dd}\u{de}\u{df}\u{fb}\u{fc}\u{ff}",
        },
    ),
    (
        "cp1256",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{20ac}\u{67e}\u{201a}\u{192}\u{201e}\u{2026}\u{2020}\u{2021}\u{2c6}\u{2030}\u{679}\u{2039}\u{152}\u{686}\u{698}\u{688}\u{6af}\u{2018}\u{2019}\u{201c}\u{201d}\u{2022}\u{2013}\u{2014}\u{6a9}\u{2122}\u{691}\u{203a}\u{153}\u{200c}\u{200d}\u{6ba}\u{a0}\u{60c}\u{a2}\u{a3}\u{a4}\u{a5}\u{a6}\u{a7}\u{a8}\u{a9}\u{6be}\u{ab}\u{ac}\u{ad}\u{ae}\u{af}\u{b0}\u{b1}\u{b2}\u{b3}\u{b4}\u{b5}\u{b6}\u{b7}\u{b8}\u{b9}\u{61b}\u{bb}\u{bc}\u{bd}\u{be}\u{61f}\u{6c1}\u{621}\u{622}\u{623}\u{624}\u{625}\u{626}\u{627}\u{628}\u{629}\u{62a}\u{62b}\u{62c}\u{62d}\u{62e}\u{62f}\u{630}\u{631}\u{632}\u{633}\u{634}\u{635}\u{636}\u{d7}\u{637}\u{638}\u{639}\u{63a}\u{640}\u{641}\u{642}\u{643}\u{e0}\u{644}\u{e2}\u{645}\u{646}\u{647}\u{648}\u{e7}\u{e8}\u{e9}\u{ea}\u{eb}\u{649}\u{64a}\u{ee}\u{ef}\u{64b}\u{64c}\u{64d}\u{64e}\u{f4}\u{64f}\u{650}\u{f7}\u{651}\u{f9}\u{652}\u{fb}\u{fc}\u{200e}\u{200f}\u{6d2}",
            undefined: "",
        },
    ),
    (
        "cp1257",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{20ac}\u{fffd}\u{201a}\u{fffd}\u{201e}\u{2026}\u{2020}\u{2021}\u{fffd}\u{2030}\u{fffd}\u{2039}\u{fffd}\u{a8}\u{2c7}\u{b8}\u{fffd}\u{2018}\u{2019}\u{201c}\u{201d}\u{2022}\u{2013}\u{2014}\u{fffd}\u{2122}\u{fffd}\u{203a}\u{fffd}\u{af}\u{2db}\u{fffd}\u{a0}\u{fffd}\u{a2}\u{a3}\u{a4}\u{fffd}\u{a6}\u{a7}\u{d8}\u{a9}\u{156}\u{ab}\u{ac}\u{ad}\u{ae}\u{c6}\u{b0}\u{b1}\u{b2}\u{b3}\u{b4}\u{b5}\u{b6}\u{b7}\u{f8}\u{b9}\u{157}\u{bb}\u{bc}\u{bd}\u{be}\u{e6}\u{104}\u{12e}\u{100}\u{106}\u{c4}\u{c5}\u{118}\u{112}\u{10c}\u{c9}\u{179}\u{116}\u{122}\u{136}\u{12a}\u{13b}\u{160}\u{143}\u{145}\u{d3}\u{14c}\u{d5}\u{d6}\u{d7}\u{172}\u{141}\u{15a}\u{16a}\u{dc}\u{17b}\u{17d}\u{df}\u{105}\u{12f}\u{101}\u{107}\u{e4}\u{e5}\u{119}\u{113}\u{10d}\u{e9}\u{17a}\u{117}\u{123}\u{137}\u{12b}\u{13c}\u{161}\u{144}\u{146}\u{f3}\u{14d}\u{f5}\u{f6}\u{f7}\u{173}\u{142}\u{15b}\u{16b}\u{fc}\u{17c}\u{17e}\u{2d9}",
            undefined: "\u{81}\u{83}\u{88}\u{8a}\u{8c}\u{90}\u{98}\u{9a}\u{9c}\u{9f}\u{a1}\u{a5}",
        },
    ),
    (
        "cp1258",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{20ac}\u{fffd}\u{201a}\u{192}\u{201e}\u{2026}\u{2020}\u{2021}\u{2c6}\u{2030}\u{fffd}\u{2039}\u{152}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{2018}\u{2019}\u{201c}\u{201d}\u{2022}\u{2013}\u{2014}\u{2dc}\u{2122}\u{fffd}\u{203a}\u{153}\u{fffd}\u{fffd}\u{178}\u{a0}\u{a1}\u{a2}\u{a3}\u{a4}\u{a5}\u{a6}\u{a7}\u{a8}\u{a9}\u{aa}\u{ab}\u{ac}\u{ad}\u{ae}\u{af}\u{b0}\u{b1}\u{b2}\u{b3}\u{b4}\u{b5}\u{b6}\u{b7}\u{b8}\u{b9}\u{ba}\u{bb}\u{bc}\u{bd}\u{be}\u{bf}\u{c0}\u{c1}\u{c2}\u{102}\u{c4}\u{c5}\u{c6}\u{c7}\u{c8}\u{c9}\u{ca}\u{cb}\u{300}\u{cd}\u{ce}\u{cf}\u{110}\u{d1}\u{309}\u{d3}\u{d4}\u{1a0}\u{d6}\u{d7}\u{d8}\u{d9}\u{da}\u{db}\u{dc}\u{1af}\u{303}\u{df}\u{e0}\u{e1}\u{e2}\u{103}\u{e4}\u{e5}\u{e6}\u{e7}\u{e8}\u{e9}\u{ea}\u{eb}\u{301}\u{ed}\u{ee}\u{ef}\u{111}\u{f1}\u{323}\u{f3}\u{f4}\u{1a1}\u{f6}\u{f7}\u{f8}\u{f9}\u{fa}\u{fb}\u{fc}\u{1b0}\u{20ab}\u{ff}",
            undefined: "\u{81}\u{8a}\u{8d}\u{8e}\u{8f}\u{90}\u{9a}\u{9d}\u{9e}",
        },
    ),
    (
        "cp437",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c7}\u{fc}\u{e9}\u{e2}\u{e4}\u{e0}\u{e5}\u{e7}\u{ea}\u{eb}\u{e8}\u{ef}\u{ee}\u{ec}\u{c4}\u{c5}\u{c9}\u{e6}\u{c6}\u{f4}\u{f6}\u{f2}\u{fb}\u{f9}\u{ff}\u{d6}\u{dc}\u{a2}\u{a3}\u{a5}\u{20a7}\u{192}\u{e1}\u{ed}\u{f3}\u{fa}\u{f1}\u{d1}\u{aa}\u{ba}\u{bf}\u{2310}\u{ac}\u{bd}\u{bc}\u{a1}\u{ab}\u{bb}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{2561}\u{2562}\u{2556}\u{2555}\u{2563}\u{2551}\u{2557}\u{255d}\u{255c}\u{255b}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{255e}\u{255f}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{2567}\u{2568}\u{2564}\u{2565}\u{2559}\u{2558}\u{2552}\u{2553}\u{256b}\u{256a}\u{2518}\u{250c}\u{2588}\u{2584}\u{258c}\u{2590}\u{2580}\u{3b1}\u{df}\u{393}\u{3c0}\u{3a3}\u{3c3}\u{b5}\u{3c4}\u{3a6}\u{398}\u{3a9}\u{3b4}\u{221e}\u{3c6}\u{3b5}\u{2229}\u{2261}\u{b1}\u{2265}\u{2264}\u{2320}\u{2321}\u{f7}\u{2248}\u{b0}\u{2219}\u{b7}\u{221a}\u{207f}\u{b2}\u{25a0}\u{a0}",
            undefined: "",
        },
    ),
    (
        "cp720",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{e9}\u{e2}\u{84}\u{e0}\u{86}\u{e7}\u{ea}\u{eb}\u{e8}\u{ef}\u{ee}\u{8d}\u{8e}\u{8f}\u{90}\u{651}\u{652}\u{f4}\u{a4}\u{640}\u{fb}\u{f9}\u{621}\u{622}\u{623}\u{624}\u{a3}\u{625}\u{626}\u{627}\u{628}\u{629}\u{62a}\u{62b}\u{62c}\u{62d}\u{62e}\u{62f}\u{630}\u{631}\u{632}\u{633}\u{634}\u{635}\u{ab}\u{bb}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{2561}\u{2562}\u{2556}\u{2555}\u{2563}\u{2551}\u{2557}\u{255d}\u{255c}\u{255b}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{255e}\u{255f}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{2567}\u{2568}\u{2564}\u{2565}\u{2559}\u{2558}\u{2552}\u{2553}\u{256b}\u{256a}\u{2518}\u{250c}\u{2588}\u{2584}\u{258c}\u{2590}\u{2580}\u{636}\u{637}\u{638}\u{639}\u{63a}\u{641}\u{b5}\u{642}\u{643}\u{644}\u{645}\u{646}\u{647}\u{648}\u{649}\u{64a}\u{2261}\u{64b}\u{64c}\u{64d}\u{64e}\u{64f}\u{650}\u{2248}\u{b0}\u{2219}\u{b7}\u{221a}\u{207f}\u{b2}\u{25a0}\u{a0}",
            undefined: "",
        },
    ),
    (
        "cp737",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{391}\u{392}\u{393}\u{394}\u{395}\u{396}\u{397}\u{398}\u{399}\u{39a}\u{39b}\u{39c}\u{39d}\u{39e}\u{39f}\u{3a0}\u{3a1}\u{3a3}\u{3a4}\u{3a5}\u{3a6}\u{3a7}\u{3a8}\u{3a9}\u{3b1}\u{3b2}\u{3b3}\u{3b4}\u{3b5}\u{3b6}\u{3b7}\u{3b8}\u{3b9}\u{3ba}\u{3bb}\u{3bc}\u{3bd}\u{3be}\u{3bf}\u{3c0}\u{3c1}\u{3c3}\u{3c2}\u{3c4}\u{3c5}\u{3c6}\u{3c7}\u{3c8}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{2561}\u{2562}\u{2556}\u{2555}\u{2563}\u{2551}\u{2557}\u{255d}\u{255c}\u{255b}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{255e}\u{255f}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{2567}\u{2568}\u{2564}\u{2565}\u{2559}\u{2558}\u{2552}\u{2553}\u{256b}\u{256a}\u{2518}\u{250c}\u{2588}\u{2584}\u{258c}\u{2590}\u{2580}\u{3c9}\u{3ac}\u{3ad}\u{3ae}\u{3ca}\u{3af}\u{3cc}\u{3cd}\u{3cb}\u{3ce}\u{386}\u{388}\u{389}\u{38a}\u{38c}\u{38e}\u{38f}\u{b1}\u{2265}\u{2264}\u{3aa}\u{3ab}\u{f7}\u{2248}\u{b0}\u{2219}\u{b7}\u{221a}\u{207f}\u{b2}\u{25a0}\u{a0}",
            undefined: "",
        },
    ),
    (
        "cp775",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{106}\u{fc}\u{e9}\u{101}\u{e4}\u{123}\u{e5}\u{107}\u{142}\u{113}\u{156}\u{157}\u{12b}\u{179}\u{c4}\u{c5}\u{c9}\u{e6}\u{c6}\u{14d}\u{f6}\u{122}\u{a2}\u{15a}\u{15b}\u{d6}\u{dc}\u{f8}\u{a3}\u{d8}\u{d7}\u{a4}\u{100}\u{12a}\u{f3}\u{17b}\u{17c}\u{17a}\u{201d}\u{a6}\u{a9}\u{ae}\u{ac}\u{bd}\u{bc}\u{141}\u{ab}\u{bb}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{104}\u{10c}\u{118}\u{116}\u{2563}\u{2551}\u{2557}\u{255d}\u{12e}\u{160}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{172}\u{16a}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{17d}\u{105}\u{10d}\u{119}\u{117}\u{12f}\u{161}\u{173}\u{16b}\u{17e}\u{2518}\u{250c}\u{2588}\u{2584}\u{258c}\u{2590}\u{2580}\u{d3}\u{df}\u{14c}\u{143}\u{f5}\u{d5}\u{b5}\u{144}\u{136}\u{137}\u{13b}\u{13c}\u{146}\u{112}\u{145}\u{2019}\u{ad}\u{b1}\u{201c}\u{be}\u{b6}\u{a7}\u{f7}\u{201e}\u{b0}\u{2219}\u{b7}\u{b9}\u{b3}\u{b2}\u{25a0}\u{a0}",
            undefined: "",
        },
    ),
    (
        "cp850",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c7}\u{fc}\u{e9}\u{e2}\u{e4}\u{e0}\u{e5}\u{e7}\u{ea}\u{eb}\u{e8}\u{ef}\u{ee}\u{ec}\u{c4}\u{c5}\u{c9}\u{e6}\u{c6}\u{f4}\u{f6}\u{f2}\u{fb}\u{f9}\u{ff}\u{d6}\u{dc}\u{f8}\u{a3}\u{d8}\u{d7}\u{192}\u{e1}\u{ed}\u{f3}\u{fa}\u{f1}\u{d1}\u{aa}\u{ba}\u{bf}\u{ae}\u{ac}\u{bd}\u{bc}\u{a1}\u{ab}\u{bb}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{c1}\u{c2}\u{c0}\u{a9}\u{2563}\u{2551}\u{2557}\u{255d}\u{a2}\u{a5}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{e3}\u{c3}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{a4}\u{f0}\u{d0}\u{ca}\u{cb}\u{c8}\u{131}\u{cd}\u{ce}\u{cf}\u{2518}\u{250c}\u{2588}\u{2584}\u{a6}\u{cc}\u{2580}\u{d3}\u{df}\u{d4}\u{d2}\u{f5}\u{d5}\u{b5}\u{fe}\u{de}\u{da}\u{db}\u{d9}\u{fd}\u{dd}\u{af}\u{b4}\u{ad}\u{b1}\u{2017}\u{be}\u{b6}\u{a7}\u{f7}\u{b8}\u{b0}\u{a8}\u{b7}\u{b9}\u{b3}\u{b2}\u{25a0}\u{a0}",
            undefined: "",
        },
    ),
    (
        "cp852",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c7}\u{fc}\u{e9}\u{e2}\u{e4}\u{16f}\u{107}\u{e7}\u{142}\u{eb}\u{150}\u{151}\u{ee}\u{179}\u{c4}\u{106}\u{c9}\u{139}\u{13a}\u{f4}\u{f6}\u{13d}\u{13e}\u{15a}\u{15b}\u{d6}\u{dc}\u{164}\u{165}\u{141}\u{d7}\u{10d}\u{e1}\u{ed}\u{f3}\u{fa}\u{104}\u{105}\u{17d}\u{17e}\u{118}\u{119}\u{ac}\u{17a}\u{10c}\u{15f}\u{ab}\u{bb}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{c1}\u{c2}\u{11a}\u{15e}\u{2563}\u{2551}\u{2557}\u{255d}\u{17b}\u{17c}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{102}\u{103}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{a4}\u{111}\u{110}\u{10e}\u{cb}\u{10f}\u{147}\u{cd}\u{ce}\u{11b}\u{2518}\u{250c}\u{2588}\u{2584}\u{162}\u{16e}\u{2580}\u{d3}\u{df}\u{d4}\u{143}\u{144}\u{148}\u{160}\u{161}\u{154}\u{da}\u{155}\u{170}\u{fd}\u{dd}\u{163}\u{b4}\u{ad}\u{2dd}\u{2db}\u{2c7}\u{2d8}\u{a7}\u{f7}\u{b8}\u{b0}\u{a8}\u{2d9}\u{171}\u{158}\u{159}\u{25a0}\u{a0}",
            undefined: "",
        },
    ),
    (
        "cp855",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{452}\u{402}\u{453}\u{403}\u{451}\u{401}\u{454}\u{404}\u{455}\u{405}\u{456}\u{406}\u{457}\u{407}\u{458}\u{408}\u{459}\u{409}\u{45a}\u{40a}\u{45b}\u{40b}\u{45c}\u{40c}\u{45e}\u{40e}\u{45f}\u{40f}\u{44e}\u{42e}\u{44a}\u{42a}\u{430}\u{410}\u{431}\u{411}\u{446}\u{426}\u{434}\u{414}\u{435}\u{415}\u{444}\u{424}\u{433}\u{413}\u{ab}\u{bb}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{445}\u{425}\u{438}\u{418}\u{2563}\u{2551}\u{2557}\u{255d}\u{439}\u{419}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{43a}\u{41a}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{a4}\u{43b}\u{41b}\u{43c}\u{41c}\u{43d}\u{41d}\u{43e}\u{41e}\u{43f}\u{2518}\u{250c}\u{2588}\u{2584}\u{41f}\u{44f}\u{2580}\u{42f}\u{440}\u{420}\u{441}\u{421}\u{442}\u{422}\u{443}\u{423}\u{436}\u{416}\u{432}\u{412}\u{44c}\u{42c}\u{2116}\u{ad}\u{44b}\u{42b}\u{437}\u{417}\u{448}\u{428}\u{44d}\u{42d}\u{449}\u{429}\u{447}\u{427}\u{a7}\u{25a0}\u{a0}",
            undefined: "",
        },
    ),
    (
        "cp856",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{5d0}\u{5d1}\u{5d2}\u{5d3}\u{5d4}\u{5d5}\u{5d6}\u{5d7}\u{5d8}\u{5d9}\u{5da}\u{5db}\u{5dc}\u{5dd}\u{5de}\u{5df}\u{5e0}\u{5e1}\u{5e2}\u{5e3}\u{5e4}\u{5e5}\u{5e6}\u{5e7}\u{5e8}\u{5e9}\u{5ea}\u{fffd}\u{a3}\u{fffd}\u{d7}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{ae}\u{ac}\u{bd}\u{bc}\u{fffd}\u{ab}\u{bb}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{fffd}\u{fffd}\u{fffd}\u{a9}\u{2563}\u{2551}\u{2557}\u{255d}\u{a2}\u{a5}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{fffd}\u{fffd}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{a4}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{2518}\u{250c}\u{2588}\u{2584}\u{a6}\u{fffd}\u{2580}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{b5}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{af}\u{b4}\u{ad}\u{b1}\u{2017}\u{be}\u{b6}\u{a7}\u{f7}\u{b8}\u{b0}\u{a8}\u{b7}\u{b9}\u{b3}\u{b2}\u{25a0}\u{a0}",
            undefined: "\u{9b}\u{9d}\u{9f}\u{a0}\u{a1}\u{a2}\u{a3}\u{a4}\u{a5}\u{a6}\u{a7}\u{a8}\u{ad}\u{b5}\u{b6}\u{b7}\u{c6}\u{c7}\u{d0}\u{d1}\u{d2}\u{d3}\u{d4}\u{d5}\u{d6}\u{d7}\u{d8}\u{de}\u{e0}\u{e1}\u{e2}\u{e3}\u{e4}\u{e5}\u{e7}\u{e8}\u{e9}\u{ea}\u{eb}\u{ec}\u{ed}",
        },
    ),
    (
        "cp857",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c7}\u{fc}\u{e9}\u{e2}\u{e4}\u{e0}\u{e5}\u{e7}\u{ea}\u{eb}\u{e8}\u{ef}\u{ee}\u{131}\u{c4}\u{c5}\u{c9}\u{e6}\u{c6}\u{f4}\u{f6}\u{f2}\u{fb}\u{f9}\u{130}\u{d6}\u{dc}\u{f8}\u{a3}\u{d8}\u{15e}\u{15f}\u{e1}\u{ed}\u{f3}\u{fa}\u{f1}\u{d1}\u{11e}\u{11f}\u{bf}\u{ae}\u{ac}\u{bd}\u{bc}\u{a1}\u{ab}\u{bb}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{c1}\u{c2}\u{c0}\u{a9}\u{2563}\u{2551}\u{2557}\u{255d}\u{a2}\u{a5}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{e3}\u{c3}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{a4}\u{ba}\u{aa}\u{ca}\u{cb}\u{c8}\u{fffd}\u{cd}\u{ce}\u{cf}\u{2518}\u{250c}\u{2588}\u{2584}\u{a6}\u{cc}\u{2580}\u{d3}\u{df}\u{d4}\u{d2}\u{f5}\u{d5}\u{b5}\u{fffd}\u{d7}\u{da}\u{db}\u{d9}\u{ec}\u{ff}\u{af}\u{b4}\u{ad}\u{b1}\u{fffd}\u{be}\u{b6}\u{a7}\u{f7}\u{b8}\u{b0}\u{a8}\u{b7}\u{b9}\u{b3}\u{b2}\u{25a0}\u{a0}",
            undefined: "\u{d5}\u{e7}\u{f2}",
        },
    ),
    (
        "cp858",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c7}\u{fc}\u{e9}\u{e2}\u{e4}\u{e0}\u{e5}\u{e7}\u{ea}\u{eb}\u{e8}\u{ef}\u{ee}\u{ec}\u{c4}\u{c5}\u{c9}\u{e6}\u{c6}\u{f4}\u{f6}\u{f2}\u{fb}\u{f9}\u{ff}\u{d6}\u{dc}\u{f8}\u{a3}\u{d8}\u{d7}\u{192}\u{e1}\u{ed}\u{f3}\u{fa}\u{f1}\u{d1}\u{aa}\u{ba}\u{bf}\u{ae}\u{ac}\u{bd}\u{bc}\u{a1}\u{ab}\u{bb}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{c1}\u{c2}\u{c0}\u{a9}\u{2563}\u{2551}\u{2557}\u{255d}\u{a2}\u{a5}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{e3}\u{c3}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{a4}\u{f0}\u{d0}\u{ca}\u{cb}\u{c8}\u{20ac}\u{cd}\u{ce}\u{cf}\u{2518}\u{250c}\u{2588}\u{2584}\u{a6}\u{cc}\u{2580}\u{d3}\u{df}\u{d4}\u{d2}\u{f5}\u{d5}\u{b5}\u{fe}\u{de}\u{da}\u{db}\u{d9}\u{fd}\u{dd}\u{af}\u{b4}\u{ad}\u{b1}\u{2017}\u{be}\u{b6}\u{a7}\u{f7}\u{b8}\u{b0}\u{a8}\u{b7}\u{b9}\u{b3}\u{b2}\u{25a0}\u{a0}",
            undefined: "",
        },
    ),
    (
        "cp860",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c7}\u{fc}\u{e9}\u{e2}\u{e3}\u{e0}\u{c1}\u{e7}\u{ea}\u{ca}\u{e8}\u{cd}\u{d4}\u{ec}\u{c3}\u{c2}\u{c9}\u{c0}\u{c8}\u{f4}\u{f5}\u{f2}\u{da}\u{f9}\u{cc}\u{d5}\u{dc}\u{a2}\u{a3}\u{d9}\u{20a7}\u{d3}\u{e1}\u{ed}\u{f3}\u{fa}\u{f1}\u{d1}\u{aa}\u{ba}\u{bf}\u{d2}\u{ac}\u{bd}\u{bc}\u{a1}\u{ab}\u{bb}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{2561}\u{2562}\u{2556}\u{2555}\u{2563}\u{2551}\u{2557}\u{255d}\u{255c}\u{255b}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{255e}\u{255f}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{2567}\u{2568}\u{2564}\u{2565}\u{2559}\u{2558}\u{2552}\u{2553}\u{256b}\u{256a}\u{2518}\u{250c}\u{2588}\u{2584}\u{258c}\u{2590}\u{2580}\u{3b1}\u{df}\u{393}\u{3c0}\u{3a3}\u{3c3}\u{b5}\u{3c4}\u{3a6}\u{398}\u{3a9}\u{3b4}\u{221e}\u{3c6}\u{3b5}\u{2229}\u{2261}\u{b1}\u{2265}\u{2264}\u{2320}\u{2321}\u{f7}\u{2248}\u{b0}\u{2219}\u{b7}\u{221a}\u{207f}\u{b2}\u{25a0}\u{a0}",
            undefined: "",
        },
    ),
    (
        "cp861",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c7}\u{fc}\u{e9}\u{e2}\u{e4}\u{e0}\u{e5}\u{e7}\u{ea}\u{eb}\u{e8}\u{d0}\u{f0}\u{de}\u{c4}\u{c5}\u{c9}\u{e6}\u{c6}\u{f4}\u{f6}\u{fe}\u{fb}\u{dd}\u{fd}\u{d6}\u{dc}\u{f8}\u{a3}\u{d8}\u{20a7}\u{192}\u{e1}\u{ed}\u{f3}\u{fa}\u{c1}\u{cd}\u{d3}\u{da}\u{bf}\u{2310}\u{ac}\u{bd}\u{bc}\u{a1}\u{ab}\u{bb}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{2561}\u{2562}\u{2556}\u{2555}\u{2563}\u{2551}\u{2557}\u{255d}\u{255c}\u{255b}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{255e}\u{255f}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{2567}\u{2568}\u{2564}\u{2565}\u{2559}\u{2558}\u{2552}\u{2553}\u{256b}\u{256a}\u{2518}\u{250c}\u{2588}\u{2584}\u{258c}\u{2590}\u{2580}\u{3b1}\u{df}\u{393}\u{3c0}\u{3a3}\u{3c3}\u{b5}\u{3c4}\u{3a6}\u{398}\u{3a9}\u{3b4}\u{221e}\u{3c6}\u{3b5}\u{2229}\u{2261}\u{b1}\u{2265}\u{2264}\u{2320}\u{2321}\u{f7}\u{2248}\u{b0}\u{2219}\u{b7}\u{221a}\u{207f}\u{b2}\u{25a0}\u{a0}",
            undefined: "",
        },
    ),
    (
        "cp862",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{5d0}\u{5d1}\u{5d2}\u{5d3}\u{5d4}\u{5d5}\u{5d6}\u{5d7}\u{5d8}\u{5d9}\u{5da}\u{5db}\u{5dc}\u{5dd}\u{5de}\u{5df}\u{5e0}\u{5e1}\u{5e2}\u{5e3}\u{5e4}\u{5e5}\u{5e6}\u{5e7}\u{5e8}\u{5e9}\u{5ea}\u{a2}\u{a3}\u{a5}\u{20a7}\u{192}\u{e1}\u{ed}\u{f3}\u{fa}\u{f1}\u{d1}\u{aa}\u{ba}\u{bf}\u{2310}\u{ac}\u{bd}\u{bc}\u{a1}\u{ab}\u{bb}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{2561}\u{2562}\u{2556}\u{2555}\u{2563}\u{2551}\u{2557}\u{255d}\u{255c}\u{255b}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{255e}\u{255f}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{2567}\u{2568}\u{2564}\u{2565}\u{2559}\u{2558}\u{2552}\u{2553}\u{256b}\u{256a}\u{2518}\u{250c}\u{2588}\u{2584}\u{258c}\u{2590}\u{2580}\u{3b1}\u{df}\u{393}\u{3c0}\u{3a3}\u{3c3}\u{b5}\u{3c4}\u{3a6}\u{398}\u{3a9}\u{3b4}\u{221e}\u{3c6}\u{3b5}\u{2229}\u{2261}\u{b1}\u{2265}\u{2264}\u{2320}\u{2321}\u{f7}\u{2248}\u{b0}\u{2219}\u{b7}\u{221a}\u{207f}\u{b2}\u{25a0}\u{a0}",
            undefined: "",
        },
    ),
    (
        "cp863",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c7}\u{fc}\u{e9}\u{e2}\u{c2}\u{e0}\u{b6}\u{e7}\u{ea}\u{eb}\u{e8}\u{ef}\u{ee}\u{2017}\u{c0}\u{a7}\u{c9}\u{c8}\u{ca}\u{f4}\u{cb}\u{cf}\u{fb}\u{f9}\u{a4}\u{d4}\u{dc}\u{a2}\u{a3}\u{d9}\u{db}\u{192}\u{a6}\u{b4}\u{f3}\u{fa}\u{a8}\u{b8}\u{b3}\u{af}\u{ce}\u{2310}\u{ac}\u{bd}\u{bc}\u{be}\u{ab}\u{bb}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{2561}\u{2562}\u{2556}\u{2555}\u{2563}\u{2551}\u{2557}\u{255d}\u{255c}\u{255b}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{255e}\u{255f}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{2567}\u{2568}\u{2564}\u{2565}\u{2559}\u{2558}\u{2552}\u{2553}\u{256b}\u{256a}\u{2518}\u{250c}\u{2588}\u{2584}\u{258c}\u{2590}\u{2580}\u{3b1}\u{df}\u{393}\u{3c0}\u{3a3}\u{3c3}\u{b5}\u{3c4}\u{3a6}\u{398}\u{3a9}\u{3b4}\u{221e}\u{3c6}\u{3b5}\u{2229}\u{2261}\u{b1}\u{2265}\u{2264}\u{2320}\u{2321}\u{f7}\u{2248}\u{b0}\u{2219}\u{b7}\u{221a}\u{207f}\u{b2}\u{25a0}\u{a0}",
            undefined: "",
        },
    ),
    (
        "cp865",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c7}\u{fc}\u{e9}\u{e2}\u{e4}\u{e0}\u{e5}\u{e7}\u{ea}\u{eb}\u{e8}\u{ef}\u{ee}\u{ec}\u{c4}\u{c5}\u{c9}\u{e6}\u{c6}\u{f4}\u{f6}\u{f2}\u{fb}\u{f9}\u{ff}\u{d6}\u{dc}\u{f8}\u{a3}\u{d8}\u{20a7}\u{192}\u{e1}\u{ed}\u{f3}\u{fa}\u{f1}\u{d1}\u{aa}\u{ba}\u{bf}\u{2310}\u{ac}\u{bd}\u{bc}\u{a1}\u{ab}\u{a4}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{2561}\u{2562}\u{2556}\u{2555}\u{2563}\u{2551}\u{2557}\u{255d}\u{255c}\u{255b}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{255e}\u{255f}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{2567}\u{2568}\u{2564}\u{2565}\u{2559}\u{2558}\u{2552}\u{2553}\u{256b}\u{256a}\u{2518}\u{250c}\u{2588}\u{2584}\u{258c}\u{2590}\u{2580}\u{3b1}\u{df}\u{393}\u{3c0}\u{3a3}\u{3c3}\u{b5}\u{3c4}\u{3a6}\u{398}\u{3a9}\u{3b4}\u{221e}\u{3c6}\u{3b5}\u{2229}\u{2261}\u{b1}\u{2265}\u{2264}\u{2320}\u{2321}\u{f7}\u{2248}\u{b0}\u{2219}\u{b7}\u{221a}\u{207f}\u{b2}\u{25a0}\u{a0}",
            undefined: "",
        },
    ),
    (
        "cp866",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{410}\u{411}\u{412}\u{413}\u{414}\u{415}\u{416}\u{417}\u{418}\u{419}\u{41a}\u{41b}\u{41c}\u{41d}\u{41e}\u{41f}\u{420}\u{421}\u{422}\u{423}\u{424}\u{425}\u{426}\u{427}\u{428}\u{429}\u{42a}\u{42b}\u{42c}\u{42d}\u{42e}\u{42f}\u{430}\u{431}\u{432}\u{433}\u{434}\u{435}\u{436}\u{437}\u{438}\u{439}\u{43a}\u{43b}\u{43c}\u{43d}\u{43e}\u{43f}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{2561}\u{2562}\u{2556}\u{2555}\u{2563}\u{2551}\u{2557}\u{255d}\u{255c}\u{255b}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{255e}\u{255f}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{2567}\u{2568}\u{2564}\u{2565}\u{2559}\u{2558}\u{2552}\u{2553}\u{256b}\u{256a}\u{2518}\u{250c}\u{2588}\u{2584}\u{258c}\u{2590}\u{2580}\u{440}\u{441}\u{442}\u{443}\u{444}\u{445}\u{446}\u{447}\u{448}\u{449}\u{44a}\u{44b}\u{44c}\u{44d}\u{44e}\u{44f}\u{401}\u{451}\u{404}\u{454}\u{407}\u{457}\u{40e}\u{45e}\u{b0}\u{2219}\u{b7}\u{221a}\u{2116}\u{a4}\u{25a0}\u{a0}",
            undefined: "",
        },
    ),
    (
        "cp869",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{386}\u{fffd}\u{b7}\u{ac}\u{a6}\u{2018}\u{2019}\u{388}\u{2015}\u{389}\u{38a}\u{3aa}\u{38c}\u{fffd}\u{fffd}\u{38e}\u{3ab}\u{a9}\u{38f}\u{b2}\u{b3}\u{3ac}\u{a3}\u{3ad}\u{3ae}\u{3af}\u{3ca}\u{390}\u{3cc}\u{3cd}\u{391}\u{392}\u{393}\u{394}\u{395}\u{396}\u{397}\u{bd}\u{398}\u{399}\u{ab}\u{bb}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{39a}\u{39b}\u{39c}\u{39d}\u{2563}\u{2551}\u{2557}\u{255d}\u{39e}\u{39f}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{3a0}\u{3a1}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{3a3}\u{3a4}\u{3a5}\u{3a6}\u{3a7}\u{3a8}\u{3a9}\u{3b1}\u{3b2}\u{3b3}\u{2518}\u{250c}\u{2588}\u{2584}\u{3b4}\u{3b5}\u{2580}\u{3b6}\u{3b7}\u{3b8}\u{3b9}\u{3ba}\u{3bb}\u{3bc}\u{3bd}\u{3be}\u{3bf}\u{3c0}\u{3c1}\u{3c3}\u{3c2}\u{3c4}\u{384}\u{ad}\u{b1}\u{3c5}\u{3c6}\u{3c7}\u{a7}\u{3c8}\u{385}\u{b0}\u{a8}\u{3c9}\u{3cb}\u{3b0}\u{3ce}\u{25a0}\u{a0}",
            undefined: "\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{87}\u{93}\u{94}",
        },
    ),
    (
        "cp874",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{20ac}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{2026}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{2018}\u{2019}\u{201c}\u{201d}\u{2022}\u{2013}\u{2014}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{a0}\u{e01}\u{e02}\u{e03}\u{e04}\u{e05}\u{e06}\u{e07}\u{e08}\u{e09}\u{e0a}\u{e0b}\u{e0c}\u{e0d}\u{e0e}\u{e0f}\u{e10}\u{e11}\u{e12}\u{e13}\u{e14}\u{e15}\u{e16}\u{e17}\u{e18}\u{e19}\u{e1a}\u{e1b}\u{e1c}\u{e1d}\u{e1e}\u{e1f}\u{e20}\u{e21}\u{e22}\u{e23}\u{e24}\u{e25}\u{e26}\u{e27}\u{e28}\u{e29}\u{e2a}\u{e2b}\u{e2c}\u{e2d}\u{e2e}\u{e2f}\u{e30}\u{e31}\u{e32}\u{e33}\u{e34}\u{e35}\u{e36}\u{e37}\u{e38}\u{e39}\u{e3a}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{e3f}\u{e40}\u{e41}\u{e42}\u{e43}\u{e44}\u{e45}\u{e46}\u{e47}\u{e48}\u{e49}\u{e4a}\u{e4b}\u{e4c}\u{e4d}\u{e4e}\u{e4f}\u{e50}\u{e51}\u{e52}\u{e53}\u{e54}\u{e55}\u{e56}\u{e57}\u{e58}\u{e59}\u{e5a}\u{e5b}\u{fffd}\u{fffd}\u{fffd}\u{fffd}",
            undefined: "\u{81}\u{82}\u{83}\u{84}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{db}\u{dc}\u{dd}\u{de}\u{fc}\u{fd}\u{fe}\u{ff}",
        },
    ),
    (
        "hp-roman8",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{c0}\u{c2}\u{c8}\u{ca}\u{cb}\u{ce}\u{cf}\u{b4}\u{2cb}\u{2c6}\u{a8}\u{2dc}\u{d9}\u{db}\u{20a4}\u{af}\u{dd}\u{fd}\u{b0}\u{c7}\u{e7}\u{d1}\u{f1}\u{a1}\u{bf}\u{a4}\u{a3}\u{a5}\u{a7}\u{192}\u{a2}\u{e2}\u{ea}\u{f4}\u{fb}\u{e1}\u{e9}\u{f3}\u{fa}\u{e0}\u{e8}\u{f2}\u{f9}\u{e4}\u{eb}\u{f6}\u{fc}\u{c5}\u{ee}\u{d8}\u{c6}\u{e5}\u{ed}\u{f8}\u{e6}\u{c4}\u{ec}\u{d6}\u{dc}\u{c9}\u{ef}\u{df}\u{d4}\u{c1}\u{c3}\u{e3}\u{d0}\u{f0}\u{cd}\u{cc}\u{d3}\u{d2}\u{d5}\u{f5}\u{160}\u{161}\u{da}\u{178}\u{ff}\u{de}\u{fe}\u{b7}\u{b5}\u{b6}\u{be}\u{2014}\u{bc}\u{bd}\u{aa}\u{ba}\u{ab}\u{25a0}\u{bb}\u{b1}\u{fffd}",
            undefined: "\u{ff}",
        },
    ),
    (
        "iso8859-1",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{a1}\u{a2}\u{a3}\u{a4}\u{a5}\u{a6}\u{a7}\u{a8}\u{a9}\u{aa}\u{ab}\u{ac}\u{ad}\u{ae}\u{af}\u{b0}\u{b1}\u{b2}\u{b3}\u{b4}\u{b5}\u{b6}\u{b7}\u{b8}\u{b9}\u{ba}\u{bb}\u{bc}\u{bd}\u{be}\u{bf}\u{c0}\u{c1}\u{c2}\u{c3}\u{c4}\u{c5}\u{c6}\u{c7}\u{c8}\u{c9}\u{ca}\u{cb}\u{cc}\u{cd}\u{ce}\u{cf}\u{d0}\u{d1}\u{d2}\u{d3}\u{d4}\u{d5}\u{d6}\u{d7}\u{d8}\u{d9}\u{da}\u{db}\u{dc}\u{dd}\u{de}\u{df}\u{e0}\u{e1}\u{e2}\u{e3}\u{e4}\u{e5}\u{e6}\u{e7}\u{e8}\u{e9}\u{ea}\u{eb}\u{ec}\u{ed}\u{ee}\u{ef}\u{f0}\u{f1}\u{f2}\u{f3}\u{f4}\u{f5}\u{f6}\u{f7}\u{f8}\u{f9}\u{fa}\u{fb}\u{fc}\u{fd}\u{fe}\u{ff}",
            undefined: "",
        },
    ),
    (
        "iso8859-10",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{104}\u{112}\u{122}\u{12a}\u{128}\u{136}\u{a7}\u{13b}\u{110}\u{160}\u{166}\u{17d}\u{ad}\u{16a}\u{14a}\u{b0}\u{105}\u{113}\u{123}\u{12b}\u{129}\u{137}\u{b7}\u{13c}\u{111}\u{161}\u{167}\u{17e}\u{2015}\u{16b}\u{14b}\u{100}\u{c1}\u{c2}\u{c3}\u{c4}\u{c5}\u{c6}\u{12e}\u{10c}\u{c9}\u{118}\u{cb}\u{116}\u{cd}\u{ce}\u{cf}\u{d0}\u{145}\u{14c}\u{d3}\u{d4}\u{d5}\u{d6}\u{168}\u{d8}\u{172}\u{da}\u{db}\u{dc}\u{dd}\u{de}\u{df}\u{101}\u{e1}\u{e2}\u{e3}\u{e4}\u{e5}\u{e6}\u{12f}\u{10d}\u{e9}\u{119}\u{eb}\u{117}\u{ed}\u{ee}\u{ef}\u{f0}\u{146}\u{14d}\u{f3}\u{f4}\u{f5}\u{f6}\u{169}\u{f8}\u{173}\u{fa}\u{fb}\u{fc}\u{fd}\u{fe}\u{138}",
            undefined: "",
        },
    ),
    (
        "iso8859-11",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{e01}\u{e02}\u{e03}\u{e04}\u{e05}\u{e06}\u{e07}\u{e08}\u{e09}\u{e0a}\u{e0b}\u{e0c}\u{e0d}\u{e0e}\u{e0f}\u{e10}\u{e11}\u{e12}\u{e13}\u{e14}\u{e15}\u{e16}\u{e17}\u{e18}\u{e19}\u{e1a}\u{e1b}\u{e1c}\u{e1d}\u{e1e}\u{e1f}\u{e20}\u{e21}\u{e22}\u{e23}\u{e24}\u{e25}\u{e26}\u{e27}\u{e28}\u{e29}\u{e2a}\u{e2b}\u{e2c}\u{e2d}\u{e2e}\u{e2f}\u{e30}\u{e31}\u{e32}\u{e33}\u{e34}\u{e35}\u{e36}\u{e37}\u{e38}\u{e39}\u{e3a}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{e3f}\u{e40}\u{e41}\u{e42}\u{e43}\u{e44}\u{e45}\u{e46}\u{e47}\u{e48}\u{e49}\u{e4a}\u{e4b}\u{e4c}\u{e4d}\u{e4e}\u{e4f}\u{e50}\u{e51}\u{e52}\u{e53}\u{e54}\u{e55}\u{e56}\u{e57}\u{e58}\u{e59}\u{e5a}\u{e5b}\u{fffd}\u{fffd}\u{fffd}\u{fffd}",
            undefined: "\u{db}\u{dc}\u{dd}\u{de}\u{fc}\u{fd}\u{fe}\u{ff}",
        },
    ),
    (
        "iso8859-13",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{201d}\u{a2}\u{a3}\u{a4}\u{201e}\u{a6}\u{a7}\u{d8}\u{a9}\u{156}\u{ab}\u{ac}\u{ad}\u{ae}\u{c6}\u{b0}\u{b1}\u{b2}\u{b3}\u{201c}\u{b5}\u{b6}\u{b7}\u{f8}\u{b9}\u{157}\u{bb}\u{bc}\u{bd}\u{be}\u{e6}\u{104}\u{12e}\u{100}\u{106}\u{c4}\u{c5}\u{118}\u{112}\u{10c}\u{c9}\u{179}\u{116}\u{122}\u{136}\u{12a}\u{13b}\u{160}\u{143}\u{145}\u{d3}\u{14c}\u{d5}\u{d6}\u{d7}\u{172}\u{141}\u{15a}\u{16a}\u{dc}\u{17b}\u{17d}\u{df}\u{105}\u{12f}\u{101}\u{107}\u{e4}\u{e5}\u{119}\u{113}\u{10d}\u{e9}\u{17a}\u{117}\u{123}\u{137}\u{12b}\u{13c}\u{161}\u{144}\u{146}\u{f3}\u{14d}\u{f5}\u{f6}\u{f7}\u{173}\u{142}\u{15b}\u{16b}\u{fc}\u{17c}\u{17e}\u{2019}",
            undefined: "",
        },
    ),
    (
        "iso8859-14",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{1e02}\u{1e03}\u{a3}\u{10a}\u{10b}\u{1e0a}\u{a7}\u{1e80}\u{a9}\u{1e82}\u{1e0b}\u{1ef2}\u{ad}\u{ae}\u{178}\u{1e1e}\u{1e1f}\u{120}\u{121}\u{1e40}\u{1e41}\u{b6}\u{1e56}\u{1e81}\u{1e57}\u{1e83}\u{1e60}\u{1ef3}\u{1e84}\u{1e85}\u{1e61}\u{c0}\u{c1}\u{c2}\u{c3}\u{c4}\u{c5}\u{c6}\u{c7}\u{c8}\u{c9}\u{ca}\u{cb}\u{cc}\u{cd}\u{ce}\u{cf}\u{174}\u{d1}\u{d2}\u{d3}\u{d4}\u{d5}\u{d6}\u{1e6a}\u{d8}\u{d9}\u{da}\u{db}\u{dc}\u{dd}\u{176}\u{df}\u{e0}\u{e1}\u{e2}\u{e3}\u{e4}\u{e5}\u{e6}\u{e7}\u{e8}\u{e9}\u{ea}\u{eb}\u{ec}\u{ed}\u{ee}\u{ef}\u{175}\u{f1}\u{f2}\u{f3}\u{f4}\u{f5}\u{f6}\u{1e6b}\u{f8}\u{f9}\u{fa}\u{fb}\u{fc}\u{fd}\u{177}\u{ff}",
            undefined: "",
        },
    ),
    (
        "iso8859-15",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{a1}\u{a2}\u{a3}\u{20ac}\u{a5}\u{160}\u{a7}\u{161}\u{a9}\u{aa}\u{ab}\u{ac}\u{ad}\u{ae}\u{af}\u{b0}\u{b1}\u{b2}\u{b3}\u{17d}\u{b5}\u{b6}\u{b7}\u{17e}\u{b9}\u{ba}\u{bb}\u{152}\u{153}\u{178}\u{bf}\u{c0}\u{c1}\u{c2}\u{c3}\u{c4}\u{c5}\u{c6}\u{c7}\u{c8}\u{c9}\u{ca}\u{cb}\u{cc}\u{cd}\u{ce}\u{cf}\u{d0}\u{d1}\u{d2}\u{d3}\u{d4}\u{d5}\u{d6}\u{d7}\u{d8}\u{d9}\u{da}\u{db}\u{dc}\u{dd}\u{de}\u{df}\u{e0}\u{e1}\u{e2}\u{e3}\u{e4}\u{e5}\u{e6}\u{e7}\u{e8}\u{e9}\u{ea}\u{eb}\u{ec}\u{ed}\u{ee}\u{ef}\u{f0}\u{f1}\u{f2}\u{f3}\u{f4}\u{f5}\u{f6}\u{f7}\u{f8}\u{f9}\u{fa}\u{fb}\u{fc}\u{fd}\u{fe}\u{ff}",
            undefined: "",
        },
    ),
    (
        "iso8859-16",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{104}\u{105}\u{141}\u{20ac}\u{201e}\u{160}\u{a7}\u{161}\u{a9}\u{218}\u{ab}\u{179}\u{ad}\u{17a}\u{17b}\u{b0}\u{b1}\u{10c}\u{142}\u{17d}\u{201d}\u{b6}\u{b7}\u{17e}\u{10d}\u{219}\u{bb}\u{152}\u{153}\u{178}\u{17c}\u{c0}\u{c1}\u{c2}\u{102}\u{c4}\u{106}\u{c6}\u{c7}\u{c8}\u{c9}\u{ca}\u{cb}\u{cc}\u{cd}\u{ce}\u{cf}\u{110}\u{143}\u{d2}\u{d3}\u{d4}\u{150}\u{d6}\u{15a}\u{170}\u{d9}\u{da}\u{db}\u{dc}\u{118}\u{21a}\u{df}\u{e0}\u{e1}\u{e2}\u{103}\u{e4}\u{107}\u{e6}\u{e7}\u{e8}\u{e9}\u{ea}\u{eb}\u{ec}\u{ed}\u{ee}\u{ef}\u{111}\u{144}\u{f2}\u{f3}\u{f4}\u{151}\u{f6}\u{15b}\u{171}\u{f9}\u{fa}\u{fb}\u{fc}\u{119}\u{21b}\u{ff}",
            undefined: "",
        },
    ),
    (
        "iso8859-2",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{104}\u{2d8}\u{141}\u{a4}\u{13d}\u{15a}\u{a7}\u{a8}\u{160}\u{15e}\u{164}\u{179}\u{ad}\u{17d}\u{17b}\u{b0}\u{105}\u{2db}\u{142}\u{b4}\u{13e}\u{15b}\u{2c7}\u{b8}\u{161}\u{15f}\u{165}\u{17a}\u{2dd}\u{17e}\u{17c}\u{154}\u{c1}\u{c2}\u{102}\u{c4}\u{139}\u{106}\u{c7}\u{10c}\u{c9}\u{118}\u{cb}\u{11a}\u{cd}\u{ce}\u{10e}\u{110}\u{143}\u{147}\u{d3}\u{d4}\u{150}\u{d6}\u{d7}\u{158}\u{16e}\u{da}\u{170}\u{dc}\u{dd}\u{162}\u{df}\u{155}\u{e1}\u{e2}\u{103}\u{e4}\u{13a}\u{107}\u{e7}\u{10d}\u{e9}\u{119}\u{eb}\u{11b}\u{ed}\u{ee}\u{10f}\u{111}\u{144}\u{148}\u{f3}\u{f4}\u{151}\u{f6}\u{f7}\u{159}\u{16f}\u{fa}\u{171}\u{fc}\u{fd}\u{163}\u{2d9}",
            undefined: "",
        },
    ),
    (
        "iso8859-3",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{126}\u{2d8}\u{a3}\u{a4}\u{fffd}\u{124}\u{a7}\u{a8}\u{130}\u{15e}\u{11e}\u{134}\u{ad}\u{fffd}\u{17b}\u{b0}\u{127}\u{b2}\u{b3}\u{b4}\u{b5}\u{125}\u{b7}\u{b8}\u{131}\u{15f}\u{11f}\u{135}\u{bd}\u{fffd}\u{17c}\u{c0}\u{c1}\u{c2}\u{fffd}\u{c4}\u{10a}\u{108}\u{c7}\u{c8}\u{c9}\u{ca}\u{cb}\u{cc}\u{cd}\u{ce}\u{cf}\u{fffd}\u{d1}\u{d2}\u{d3}\u{d4}\u{120}\u{d6}\u{d7}\u{11c}\u{d9}\u{da}\u{db}\u{dc}\u{16c}\u{15c}\u{df}\u{e0}\u{e1}\u{e2}\u{fffd}\u{e4}\u{10b}\u{109}\u{e7}\u{e8}\u{e9}\u{ea}\u{eb}\u{ec}\u{ed}\u{ee}\u{ef}\u{fffd}\u{f1}\u{f2}\u{f3}\u{f4}\u{121}\u{f6}\u{f7}\u{11d}\u{f9}\u{fa}\u{fb}\u{fc}\u{16d}\u{15d}\u{2d9}",
            undefined: "\u{a5}\u{ae}\u{be}\u{c3}\u{d0}\u{e3}\u{f0}",
        },
    ),
    (
        "iso8859-4",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{104}\u{138}\u{156}\u{a4}\u{128}\u{13b}\u{a7}\u{a8}\u{160}\u{112}\u{122}\u{166}\u{ad}\u{17d}\u{af}\u{b0}\u{105}\u{2db}\u{157}\u{b4}\u{129}\u{13c}\u{2c7}\u{b8}\u{161}\u{113}\u{123}\u{167}\u{14a}\u{17e}\u{14b}\u{100}\u{c1}\u{c2}\u{c3}\u{c4}\u{c5}\u{c6}\u{12e}\u{10c}\u{c9}\u{118}\u{cb}\u{116}\u{cd}\u{ce}\u{12a}\u{110}\u{145}\u{14c}\u{136}\u{d4}\u{d5}\u{d6}\u{d7}\u{d8}\u{172}\u{da}\u{db}\u{dc}\u{168}\u{16a}\u{df}\u{101}\u{e1}\u{e2}\u{e3}\u{e4}\u{e5}\u{e6}\u{12f}\u{10d}\u{e9}\u{119}\u{eb}\u{117}\u{ed}\u{ee}\u{12b}\u{111}\u{146}\u{14d}\u{137}\u{f4}\u{f5}\u{f6}\u{f7}\u{f8}\u{173}\u{fa}\u{fb}\u{fc}\u{169}\u{16b}\u{2d9}",
            undefined: "",
        },
    ),
    (
        "iso8859-5",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{401}\u{402}\u{403}\u{404}\u{405}\u{406}\u{407}\u{408}\u{409}\u{40a}\u{40b}\u{40c}\u{ad}\u{40e}\u{40f}\u{410}\u{411}\u{412}\u{413}\u{414}\u{415}\u{416}\u{417}\u{418}\u{419}\u{41a}\u{41b}\u{41c}\u{41d}\u{41e}\u{41f}\u{420}\u{421}\u{422}\u{423}\u{424}\u{425}\u{426}\u{427}\u{428}\u{429}\u{42a}\u{42b}\u{42c}\u{42d}\u{42e}\u{42f}\u{430}\u{431}\u{432}\u{433}\u{434}\u{435}\u{436}\u{437}\u{438}\u{439}\u{43a}\u{43b}\u{43c}\u{43d}\u{43e}\u{43f}\u{440}\u{441}\u{442}\u{443}\u{444}\u{445}\u{446}\u{447}\u{448}\u{449}\u{44a}\u{44b}\u{44c}\u{44d}\u{44e}\u{44f}\u{2116}\u{451}\u{452}\u{453}\u{454}\u{455}\u{456}\u{457}\u{458}\u{459}\u{45a}\u{45b}\u{45c}\u{a7}\u{45e}\u{45f}",
            undefined: "",
        },
    ),
    (
        "iso8859-6",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{fffd}\u{fffd}\u{fffd}\u{a4}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{60c}\u{ad}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{61b}\u{fffd}\u{fffd}\u{fffd}\u{61f}\u{fffd}\u{621}\u{622}\u{623}\u{624}\u{625}\u{626}\u{627}\u{628}\u{629}\u{62a}\u{62b}\u{62c}\u{62d}\u{62e}\u{62f}\u{630}\u{631}\u{632}\u{633}\u{634}\u{635}\u{636}\u{637}\u{638}\u{639}\u{63a}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{640}\u{641}\u{642}\u{643}\u{644}\u{645}\u{646}\u{647}\u{648}\u{649}\u{64a}\u{64b}\u{64c}\u{64d}\u{64e}\u{64f}\u{650}\u{651}\u{652}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}",
            undefined: "\u{a1}\u{a2}\u{a3}\u{a5}\u{a6}\u{a7}\u{a8}\u{a9}\u{aa}\u{ab}\u{ae}\u{af}\u{b0}\u{b1}\u{b2}\u{b3}\u{b4}\u{b5}\u{b6}\u{b7}\u{b8}\u{b9}\u{ba}\u{bc}\u{bd}\u{be}\u{c0}\u{db}\u{dc}\u{dd}\u{de}\u{df}\u{f3}\u{f4}\u{f5}\u{f6}\u{f7}\u{f8}\u{f9}\u{fa}\u{fb}\u{fc}\u{fd}\u{fe}\u{ff}",
        },
    ),
    (
        "iso8859-7",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{2018}\u{2019}\u{a3}\u{20ac}\u{20af}\u{a6}\u{a7}\u{a8}\u{a9}\u{37a}\u{ab}\u{ac}\u{ad}\u{fffd}\u{2015}\u{b0}\u{b1}\u{b2}\u{b3}\u{384}\u{385}\u{386}\u{b7}\u{388}\u{389}\u{38a}\u{bb}\u{38c}\u{bd}\u{38e}\u{38f}\u{390}\u{391}\u{392}\u{393}\u{394}\u{395}\u{396}\u{397}\u{398}\u{399}\u{39a}\u{39b}\u{39c}\u{39d}\u{39e}\u{39f}\u{3a0}\u{3a1}\u{fffd}\u{3a3}\u{3a4}\u{3a5}\u{3a6}\u{3a7}\u{3a8}\u{3a9}\u{3aa}\u{3ab}\u{3ac}\u{3ad}\u{3ae}\u{3af}\u{3b0}\u{3b1}\u{3b2}\u{3b3}\u{3b4}\u{3b5}\u{3b6}\u{3b7}\u{3b8}\u{3b9}\u{3ba}\u{3bb}\u{3bc}\u{3bd}\u{3be}\u{3bf}\u{3c0}\u{3c1}\u{3c2}\u{3c3}\u{3c4}\u{3c5}\u{3c6}\u{3c7}\u{3c8}\u{3c9}\u{3ca}\u{3cb}\u{3cc}\u{3cd}\u{3ce}\u{fffd}",
            undefined: "\u{ae}\u{d2}\u{ff}",
        },
    ),
    (
        "iso8859-8",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{fffd}\u{a2}\u{a3}\u{a4}\u{a5}\u{a6}\u{a7}\u{a8}\u{a9}\u{d7}\u{ab}\u{ac}\u{ad}\u{ae}\u{af}\u{b0}\u{b1}\u{b2}\u{b3}\u{b4}\u{b5}\u{b6}\u{b7}\u{b8}\u{b9}\u{f7}\u{bb}\u{bc}\u{bd}\u{be}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{2017}\u{5d0}\u{5d1}\u{5d2}\u{5d3}\u{5d4}\u{5d5}\u{5d6}\u{5d7}\u{5d8}\u{5d9}\u{5da}\u{5db}\u{5dc}\u{5dd}\u{5de}\u{5df}\u{5e0}\u{5e1}\u{5e2}\u{5e3}\u{5e4}\u{5e5}\u{5e6}\u{5e7}\u{5e8}\u{5e9}\u{5ea}\u{fffd}\u{fffd}\u{200e}\u{200f}\u{fffd}",
            undefined: "\u{a1}\u{bf}\u{c0}\u{c1}\u{c2}\u{c3}\u{c4}\u{c5}\u{c6}\u{c7}\u{c8}\u{c9}\u{ca}\u{cb}\u{cc}\u{cd}\u{ce}\u{cf}\u{d0}\u{d1}\u{d2}\u{d3}\u{d4}\u{d5}\u{d6}\u{d7}\u{d8}\u{d9}\u{da}\u{db}\u{dc}\u{dd}\u{de}\u{fb}\u{fc}\u{ff}",
        },
    ),
    (
        "iso8859-9",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{a1}\u{a2}\u{a3}\u{a4}\u{a5}\u{a6}\u{a7}\u{a8}\u{a9}\u{aa}\u{ab}\u{ac}\u{ad}\u{ae}\u{af}\u{b0}\u{b1}\u{b2}\u{b3}\u{b4}\u{b5}\u{b6}\u{b7}\u{b8}\u{b9}\u{ba}\u{bb}\u{bc}\u{bd}\u{be}\u{bf}\u{c0}\u{c1}\u{c2}\u{c3}\u{c4}\u{c5}\u{c6}\u{c7}\u{c8}\u{c9}\u{ca}\u{cb}\u{cc}\u{cd}\u{ce}\u{cf}\u{11e}\u{d1}\u{d2}\u{d3}\u{d4}\u{d5}\u{d6}\u{d7}\u{d8}\u{d9}\u{da}\u{db}\u{dc}\u{130}\u{15e}\u{df}\u{e0}\u{e1}\u{e2}\u{e3}\u{e4}\u{e5}\u{e6}\u{e7}\u{e8}\u{e9}\u{ea}\u{eb}\u{ec}\u{ed}\u{ee}\u{ef}\u{11f}\u{f1}\u{f2}\u{f3}\u{f4}\u{f5}\u{f6}\u{f7}\u{f8}\u{f9}\u{fa}\u{fb}\u{fc}\u{131}\u{15f}\u{ff}",
            undefined: "",
        },
    ),
    (
        "koi8-r",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{2500}\u{2502}\u{250c}\u{2510}\u{2514}\u{2518}\u{251c}\u{2524}\u{252c}\u{2534}\u{253c}\u{2580}\u{2584}\u{2588}\u{258c}\u{2590}\u{2591}\u{2592}\u{2593}\u{2320}\u{25a0}\u{2219}\u{221a}\u{2248}\u{2264}\u{2265}\u{a0}\u{2321}\u{b0}\u{b2}\u{b7}\u{f7}\u{2550}\u{2551}\u{2552}\u{451}\u{2553}\u{2554}\u{2555}\u{2556}\u{2557}\u{2558}\u{2559}\u{255a}\u{255b}\u{255c}\u{255d}\u{255e}\u{255f}\u{2560}\u{2561}\u{401}\u{2562}\u{2563}\u{2564}\u{2565}\u{2566}\u{2567}\u{2568}\u{2569}\u{256a}\u{256b}\u{256c}\u{a9}\u{44e}\u{430}\u{431}\u{446}\u{434}\u{435}\u{444}\u{433}\u{445}\u{438}\u{439}\u{43a}\u{43b}\u{43c}\u{43d}\u{43e}\u{43f}\u{44f}\u{440}\u{441}\u{442}\u{443}\u{436}\u{432}\u{44c}\u{44b}\u{437}\u{448}\u{44d}\u{449}\u{447}\u{44a}\u{42e}\u{410}\u{411}\u{426}\u{414}\u{415}\u{424}\u{413}\u{425}\u{418}\u{419}\u{41a}\u{41b}\u{41c}\u{41d}\u{41e}\u{41f}\u{42f}\u{420}\u{421}\u{422}\u{423}\u{416}\u{412}\u{42c}\u{42b}\u{417}\u{428}\u{42d}\u{429}\u{427}\u{42a}",
            undefined: "",
        },
    ),
    (
        "koi8-t",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{49b}\u{493}\u{201a}\u{492}\u{201e}\u{2026}\u{2020}\u{2021}\u{fffd}\u{2030}\u{4b3}\u{2039}\u{4b2}\u{4b7}\u{4b6}\u{fffd}\u{49a}\u{2018}\u{2019}\u{201c}\u{201d}\u{2022}\u{2013}\u{2014}\u{fffd}\u{2122}\u{fffd}\u{203a}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{4ef}\u{4ee}\u{451}\u{a4}\u{4e3}\u{a6}\u{a7}\u{fffd}\u{fffd}\u{fffd}\u{ab}\u{ac}\u{ad}\u{ae}\u{fffd}\u{b0}\u{b1}\u{b2}\u{401}\u{fffd}\u{4e2}\u{b6}\u{b7}\u{fffd}\u{2116}\u{fffd}\u{bb}\u{fffd}\u{fffd}\u{fffd}\u{a9}\u{44e}\u{430}\u{431}\u{446}\u{434}\u{435}\u{444}\u{433}\u{445}\u{438}\u{439}\u{43a}\u{43b}\u{43c}\u{43d}\u{43e}\u{43f}\u{44f}\u{440}\u{441}\u{442}\u{443}\u{436}\u{432}\u{44c}\u{44b}\u{437}\u{448}\u{44d}\u{449}\u{447}\u{44a}\u{42e}\u{410}\u{411}\u{426}\u{414}\u{415}\u{424}\u{413}\u{425}\u{418}\u{419}\u{41a}\u{41b}\u{41c}\u{41d}\u{41e}\u{41f}\u{42f}\u{420}\u{421}\u{422}\u{423}\u{416}\u{412}\u{42c}\u{42b}\u{417}\u{428}\u{42d}\u{429}\u{427}\u{42a}",
            undefined: "\u{88}\u{8f}\u{98}\u{9a}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{a8}\u{a9}\u{aa}\u{af}\u{b4}\u{b8}\u{ba}\u{bc}\u{bd}\u{be}",
        },
    ),
    (
        "koi8-u",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{2500}\u{2502}\u{250c}\u{2510}\u{2514}\u{2518}\u{251c}\u{2524}\u{252c}\u{2534}\u{253c}\u{2580}\u{2584}\u{2588}\u{258c}\u{2590}\u{2591}\u{2592}\u{2593}\u{2320}\u{25a0}\u{2219}\u{221a}\u{2248}\u{2264}\u{2265}\u{a0}\u{2321}\u{b0}\u{b2}\u{b7}\u{f7}\u{2550}\u{2551}\u{2552}\u{451}\u{454}\u{2554}\u{456}\u{457}\u{2557}\u{2558}\u{2559}\u{255a}\u{255b}\u{491}\u{255d}\u{255e}\u{255f}\u{2560}\u{2561}\u{401}\u{404}\u{2563}\u{406}\u{407}\u{2566}\u{2567}\u{2568}\u{2569}\u{256a}\u{490}\u{256c}\u{a9}\u{44e}\u{430}\u{431}\u{446}\u{434}\u{435}\u{444}\u{433}\u{445}\u{438}\u{439}\u{43a}\u{43b}\u{43c}\u{43d}\u{43e}\u{43f}\u{44f}\u{440}\u{441}\u{442}\u{443}\u{436}\u{432}\u{44c}\u{44b}\u{437}\u{448}\u{44d}\u{449}\u{447}\u{44a}\u{42e}\u{410}\u{411}\u{426}\u{414}\u{415}\u{424}\u{413}\u{425}\u{418}\u{419}\u{41a}\u{41b}\u{41c}\u{41d}\u{41e}\u{41f}\u{42f}\u{420}\u{421}\u{422}\u{423}\u{416}\u{412}\u{42c}\u{42b}\u{417}\u{428}\u{42d}\u{429}\u{427}\u{42a}",
            undefined: "",
        },
    ),
    (
        "kz1048",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{402}\u{403}\u{201a}\u{453}\u{201e}\u{2026}\u{2020}\u{2021}\u{20ac}\u{2030}\u{409}\u{2039}\u{40a}\u{49a}\u{4ba}\u{40f}\u{452}\u{2018}\u{2019}\u{201c}\u{201d}\u{2022}\u{2013}\u{2014}\u{fffd}\u{2122}\u{459}\u{203a}\u{45a}\u{49b}\u{4bb}\u{45f}\u{a0}\u{4b0}\u{4b1}\u{4d8}\u{a4}\u{4e8}\u{a6}\u{a7}\u{401}\u{a9}\u{492}\u{ab}\u{ac}\u{ad}\u{ae}\u{4ae}\u{b0}\u{b1}\u{406}\u{456}\u{4e9}\u{b5}\u{b6}\u{b7}\u{451}\u{2116}\u{493}\u{bb}\u{4d9}\u{4a2}\u{4a3}\u{4af}\u{410}\u{411}\u{412}\u{413}\u{414}\u{415}\u{416}\u{417}\u{418}\u{419}\u{41a}\u{41b}\u{41c}\u{41d}\u{41e}\u{41f}\u{420}\u{421}\u{422}\u{423}\u{424}\u{425}\u{426}\u{427}\u{428}\u{429}\u{42a}\u{42b}\u{42c}\u{42d}\u{42e}\u{42f}\u{430}\u{431}\u{432}\u{433}\u{434}\u{435}\u{436}\u{437}\u{438}\u{439}\u{43a}\u{43b}\u{43c}\u{43d}\u{43e}\u{43f}\u{440}\u{441}\u{442}\u{443}\u{444}\u{445}\u{446}\u{447}\u{448}\u{449}\u{44a}\u{44b}\u{44c}\u{44d}\u{44e}\u{44f}",
            undefined: "\u{98}",
        },
    ),
    (
        "mac-arabic",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c4}\u{a0}\u{c7}\u{c9}\u{d1}\u{d6}\u{dc}\u{e1}\u{e0}\u{e2}\u{e4}\u{6ba}\u{ab}\u{e7}\u{e9}\u{e8}\u{ea}\u{eb}\u{ed}\u{2026}\u{ee}\u{ef}\u{f1}\u{f3}\u{bb}\u{f4}\u{f6}\u{f7}\u{fa}\u{f9}\u{fb}\u{fc} !\"#$\u{66a}&'()*+\u{60c}-./\u{660}\u{661}\u{662}\u{663}\u{664}\u{665}\u{666}\u{667}\u{668}\u{669}:\u{61b}<=>\u{61f}\u{274a}\u{621}\u{622}\u{623}\u{624}\u{625}\u{626}\u{627}\u{628}\u{629}\u{62a}\u{62b}\u{62c}\u{62d}\u{62e}\u{62f}\u{630}\u{631}\u{632}\u{633}\u{634}\u{635}\u{636}\u{637}\u{638}\u{639}\u{63a}[\\]^_\u{640}\u{641}\u{642}\u{643}\u{644}\u{645}\u{646}\u{647}\u{648}\u{649}\u{64a}\u{64b}\u{64c}\u{64d}\u{64e}\u{64f}\u{650}\u{651}\u{652}\u{67e}\u{679}\u{686}\u{6d5}\u{6a4}\u{6af}\u{688}\u{691}{|}\u{698}\u{6d2}",
            undefined: "",
        },
    ),
    (
        "mac-croatian",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c4}\u{c5}\u{c7}\u{c9}\u{d1}\u{d6}\u{dc}\u{e1}\u{e0}\u{e2}\u{e4}\u{e3}\u{e5}\u{e7}\u{e9}\u{e8}\u{ea}\u{eb}\u{ed}\u{ec}\u{ee}\u{ef}\u{f1}\u{f3}\u{f2}\u{f4}\u{f6}\u{f5}\u{fa}\u{f9}\u{fb}\u{fc}\u{2020}\u{b0}\u{a2}\u{a3}\u{a7}\u{2022}\u{b6}\u{df}\u{ae}\u{160}\u{2122}\u{b4}\u{a8}\u{2260}\u{17d}\u{d8}\u{221e}\u{b1}\u{2264}\u{2265}\u{2206}\u{b5}\u{2202}\u{2211}\u{220f}\u{161}\u{222b}\u{aa}\u{ba}\u{3a9}\u{17e}\u{f8}\u{bf}\u{a1}\u{ac}\u{221a}\u{192}\u{2248}\u{106}\u{ab}\u{10c}\u{2026}\u{a0}\u{c0}\u{c3}\u{d5}\u{152}\u{153}\u{110}\u{2014}\u{201c}\u{201d}\u{2018}\u{2019}\u{f7}\u{25ca}\u{f8ff}\u{a9}\u{2044}\u{20ac}\u{2039}\u{203a}\u{c6}\u{bb}\u{2013}\u{b7}\u{201a}\u{201e}\u{2030}\u{c2}\u{107}\u{c1}\u{10d}\u{c8}\u{cd}\u{ce}\u{cf}\u{cc}\u{d3}\u{d4}\u{111}\u{d2}\u{da}\u{db}\u{d9}\u{131}\u{2c6}\u{2dc}\u{af}\u{3c0}\u{cb}\u{2da}\u{b8}\u{ca}\u{e6}\u{2c7}",
            undefined: "",
        },
    ),
    (
        "mac-cyrillic",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{410}\u{411}\u{412}\u{413}\u{414}\u{415}\u{416}\u{417}\u{418}\u{419}\u{41a}\u{41b}\u{41c}\u{41d}\u{41e}\u{41f}\u{420}\u{421}\u{422}\u{423}\u{424}\u{425}\u{426}\u{427}\u{428}\u{429}\u{42a}\u{42b}\u{42c}\u{42d}\u{42e}\u{42f}\u{2020}\u{b0}\u{490}\u{a3}\u{a7}\u{2022}\u{b6}\u{406}\u{ae}\u{a9}\u{2122}\u{402}\u{452}\u{2260}\u{403}\u{453}\u{221e}\u{b1}\u{2264}\u{2265}\u{456}\u{b5}\u{491}\u{408}\u{404}\u{454}\u{407}\u{457}\u{409}\u{459}\u{40a}\u{45a}\u{458}\u{405}\u{ac}\u{221a}\u{192}\u{2248}\u{2206}\u{ab}\u{bb}\u{2026}\u{a0}\u{40b}\u{45b}\u{40c}\u{45c}\u{455}\u{2013}\u{2014}\u{201c}\u{201d}\u{2018}\u{2019}\u{f7}\u{201e}\u{40e}\u{45e}\u{40f}\u{45f}\u{2116}\u{401}\u{451}\u{44f}\u{430}\u{431}\u{432}\u{433}\u{434}\u{435}\u{436}\u{437}\u{438}\u{439}\u{43a}\u{43b}\u{43c}\u{43d}\u{43e}\u{43f}\u{440}\u{441}\u{442}\u{443}\u{444}\u{445}\u{446}\u{447}\u{448}\u{449}\u{44a}\u{44b}\u{44c}\u{44d}\u{44e}\u{20ac}",
            undefined: "",
        },
    ),
    (
        "mac-farsi",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c4}\u{a0}\u{c7}\u{c9}\u{d1}\u{d6}\u{dc}\u{e1}\u{e0}\u{e2}\u{e4}\u{6ba}\u{ab}\u{e7}\u{e9}\u{e8}\u{ea}\u{eb}\u{ed}\u{2026}\u{ee}\u{ef}\u{f1}\u{f3}\u{bb}\u{f4}\u{f6}\u{f7}\u{fa}\u{f9}\u{fb}\u{fc} !\"#$\u{66a}&'()*+\u{60c}-./\u{6f0}\u{6f1}\u{6f2}\u{6f3}\u{6f4}\u{6f5}\u{6f6}\u{6f7}\u{6f8}\u{6f9}:\u{61b}<=>\u{61f}\u{274a}\u{621}\u{622}\u{623}\u{624}\u{625}\u{626}\u{627}\u{628}\u{629}\u{62a}\u{62b}\u{62c}\u{62d}\u{62e}\u{62f}\u{630}\u{631}\u{632}\u{633}\u{634}\u{635}\u{636}\u{637}\u{638}\u{639}\u{63a}[\\]^_\u{640}\u{641}\u{642}\u{643}\u{644}\u{645}\u{646}\u{647}\u{648}\u{649}\u{64a}\u{64b}\u{64c}\u{64d}\u{64e}\u{64f}\u{650}\u{651}\u{652}\u{67e}\u{679}\u{686}\u{6d5}\u{6a4}\u{6af}\u{688}\u{691}{|}\u{698}\u{6d2}",
            undefined: "",
        },
    ),
    (
        "mac-greek",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c4}\u{b9}\u{b2}\u{c9}\u{b3}\u{d6}\u{dc}\u{385}\u{e0}\u{e2}\u{e4}\u{384}\u{a8}\u{e7}\u{e9}\u{e8}\u{ea}\u{eb}\u{a3}\u{2122}\u{ee}\u{ef}\u{2022}\u{bd}\u{2030}\u{f4}\u{f6}\u{a6}\u{20ac}\u{f9}\u{fb}\u{fc}\u{2020}\u{393}\u{394}\u{398}\u{39b}\u{39e}\u{3a0}\u{df}\u{ae}\u{a9}\u{3a3}\u{3aa}\u{a7}\u{2260}\u{b0}\u{b7}\u{391}\u{b1}\u{2264}\u{2265}\u{a5}\u{392}\u{395}\u{396}\u{397}\u{399}\u{39a}\u{39c}\u{3a6}\u{3ab}\u{3a8}\u{3a9}\u{3ac}\u{39d}\u{ac}\u{39f}\u{3a1}\u{2248}\u{3a4}\u{ab}\u{bb}\u{2026}\u{a0}\u{3a5}\u{3a7}\u{386}\u{388}\u{153}\u{2013}\u{2015}\u{201c}\u{201d}\u{2018}\u{2019}\u{f7}\u{389}\u{38a}\u{38c}\u{38e}\u{3ad}\u{3ae}\u{3af}\u{3cc}\u{38f}\u{3cd}\u{3b1}\u{3b2}\u{3c8}\u{3b4}\u{3b5}\u{3c6}\u{3b3}\u{3b7}\u{3b9}\u{3be}\u{3ba}\u{3bb}\u{3bc}\u{3bd}\u{3bf}\u{3c0}\u{3ce}\u{3c1}\u{3c3}\u{3c4}\u{3b8}\u{3c9}\u{3c2}\u{3c7}\u{3c5}\u{3b6}\u{3ca}\u{3cb}\u{390}\u{3b0}\u{ad}",
            undefined: "",
        },
    ),
    (
        "mac-iceland",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c4}\u{c5}\u{c7}\u{c9}\u{d1}\u{d6}\u{dc}\u{e1}\u{e0}\u{e2}\u{e4}\u{e3}\u{e5}\u{e7}\u{e9}\u{e8}\u{ea}\u{eb}\u{ed}\u{ec}\u{ee}\u{ef}\u{f1}\u{f3}\u{f2}\u{f4}\u{f6}\u{f5}\u{fa}\u{f9}\u{fb}\u{fc}\u{dd}\u{b0}\u{a2}\u{a3}\u{a7}\u{2022}\u{b6}\u{df}\u{ae}\u{a9}\u{2122}\u{b4}\u{a8}\u{2260}\u{c6}\u{d8}\u{221e}\u{b1}\u{2264}\u{2265}\u{a5}\u{b5}\u{2202}\u{2211}\u{220f}\u{3c0}\u{222b}\u{aa}\u{ba}\u{3a9}\u{e6}\u{f8}\u{bf}\u{a1}\u{ac}\u{221a}\u{192}\u{2248}\u{2206}\u{ab}\u{bb}\u{2026}\u{a0}\u{c0}\u{c3}\u{d5}\u{152}\u{153}\u{2013}\u{2014}\u{201c}\u{201d}\u{2018}\u{2019}\u{f7}\u{25ca}\u{ff}\u{178}\u{2044}\u{20ac}\u{d0}\u{f0}\u{de}\u{fe}\u{fd}\u{b7}\u{201a}\u{201e}\u{2030}\u{c2}\u{ca}\u{c1}\u{cb}\u{c8}\u{cd}\u{ce}\u{cf}\u{cc}\u{d3}\u{d4}\u{f8ff}\u{d2}\u{da}\u{db}\u{d9}\u{131}\u{2c6}\u{2dc}\u{af}\u{2d8}\u{2d9}\u{2da}\u{b8}\u{2dd}\u{2db}\u{2c7}",
            undefined: "",
        },
    ),
    (
        "mac-latin2",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c4}\u{100}\u{101}\u{c9}\u{104}\u{d6}\u{dc}\u{e1}\u{105}\u{10c}\u{e4}\u{10d}\u{106}\u{107}\u{e9}\u{179}\u{17a}\u{10e}\u{ed}\u{10f}\u{112}\u{113}\u{116}\u{f3}\u{117}\u{f4}\u{f6}\u{f5}\u{fa}\u{11a}\u{11b}\u{fc}\u{2020}\u{b0}\u{118}\u{a3}\u{a7}\u{2022}\u{b6}\u{df}\u{ae}\u{a9}\u{2122}\u{119}\u{a8}\u{2260}\u{123}\u{12e}\u{12f}\u{12a}\u{2264}\u{2265}\u{12b}\u{136}\u{2202}\u{2211}\u{142}\u{13b}\u{13c}\u{13d}\u{13e}\u{139}\u{13a}\u{145}\u{146}\u{143}\u{ac}\u{221a}\u{144}\u{147}\u{2206}\u{ab}\u{bb}\u{2026}\u{a0}\u{148}\u{150}\u{d5}\u{151}\u{14c}\u{2013}\u{2014}\u{201c}\u{201d}\u{2018}\u{2019}\u{f7}\u{25ca}\u{14d}\u{154}\u{155}\u{158}\u{2039}\u{203a}\u{159}\u{156}\u{157}\u{160}\u{201a}\u{201e}\u{161}\u{15a}\u{15b}\u{c1}\u{164}\u{165}\u{cd}\u{17d}\u{17e}\u{16a}\u{d3}\u{d4}\u{16b}\u{16e}\u{da}\u{16f}\u{170}\u{171}\u{172}\u{173}\u{dd}\u{fd}\u{137}\u{17b}\u{141}\u{17c}\u{122}\u{2c7}",
            undefined: "",
        },
    ),
    (
        "mac-roman",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c4}\u{c5}\u{c7}\u{c9}\u{d1}\u{d6}\u{dc}\u{e1}\u{e0}\u{e2}\u{e4}\u{e3}\u{e5}\u{e7}\u{e9}\u{e8}\u{ea}\u{eb}\u{ed}\u{ec}\u{ee}\u{ef}\u{f1}\u{f3}\u{f2}\u{f4}\u{f6}\u{f5}\u{fa}\u{f9}\u{fb}\u{fc}\u{2020}\u{b0}\u{a2}\u{a3}\u{a7}\u{2022}\u{b6}\u{df}\u{ae}\u{a9}\u{2122}\u{b4}\u{a8}\u{2260}\u{c6}\u{d8}\u{221e}\u{b1}\u{2264}\u{2265}\u{a5}\u{b5}\u{2202}\u{2211}\u{220f}\u{3c0}\u{222b}\u{aa}\u{ba}\u{3a9}\u{e6}\u{f8}\u{bf}\u{a1}\u{ac}\u{221a}\u{192}\u{2248}\u{2206}\u{ab}\u{bb}\u{2026}\u{a0}\u{c0}\u{c3}\u{d5}\u{152}\u{153}\u{2013}\u{2014}\u{201c}\u{201d}\u{2018}\u{2019}\u{f7}\u{25ca}\u{ff}\u{178}\u{2044}\u{20ac}\u{2039}\u{203a}\u{fb01}\u{fb02}\u{2021}\u{b7}\u{201a}\u{201e}\u{2030}\u{c2}\u{ca}\u{c1}\u{cb}\u{c8}\u{cd}\u{ce}\u{cf}\u{cc}\u{d3}\u{d4}\u{f8ff}\u{d2}\u{da}\u{db}\u{d9}\u{131}\u{2c6}\u{2dc}\u{af}\u{2d8}\u{2d9}\u{2da}\u{b8}\u{2dd}\u{2db}\u{2c7}",
            undefined: "",
        },
    ),
    (
        "mac-romanian",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c4}\u{c5}\u{c7}\u{c9}\u{d1}\u{d6}\u{dc}\u{e1}\u{e0}\u{e2}\u{e4}\u{e3}\u{e5}\u{e7}\u{e9}\u{e8}\u{ea}\u{eb}\u{ed}\u{ec}\u{ee}\u{ef}\u{f1}\u{f3}\u{f2}\u{f4}\u{f6}\u{f5}\u{fa}\u{f9}\u{fb}\u{fc}\u{2020}\u{b0}\u{a2}\u{a3}\u{a7}\u{2022}\u{b6}\u{df}\u{ae}\u{a9}\u{2122}\u{b4}\u{a8}\u{2260}\u{102}\u{218}\u{221e}\u{b1}\u{2264}\u{2265}\u{a5}\u{b5}\u{2202}\u{2211}\u{220f}\u{3c0}\u{222b}\u{aa}\u{ba}\u{3a9}\u{103}\u{219}\u{bf}\u{a1}\u{ac}\u{221a}\u{192}\u{2248}\u{2206}\u{ab}\u{bb}\u{2026}\u{a0}\u{c0}\u{c3}\u{d5}\u{152}\u{153}\u{2013}\u{2014}\u{201c}\u{201d}\u{2018}\u{2019}\u{f7}\u{25ca}\u{ff}\u{178}\u{2044}\u{20ac}\u{2039}\u{203a}\u{21a}\u{21b}\u{2021}\u{b7}\u{201a}\u{201e}\u{2030}\u{c2}\u{ca}\u{c1}\u{cb}\u{c8}\u{cd}\u{ce}\u{cf}\u{cc}\u{d3}\u{d4}\u{f8ff}\u{d2}\u{da}\u{db}\u{d9}\u{131}\u{2c6}\u{2dc}\u{af}\u{2d8}\u{2d9}\u{2da}\u{b8}\u{2dd}\u{2db}\u{2c7}",
            undefined: "",
        },
    ),
    (
        "mac-turkish",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{c4}\u{c5}\u{c7}\u{c9}\u{d1}\u{d6}\u{dc}\u{e1}\u{e0}\u{e2}\u{e4}\u{e3}\u{e5}\u{e7}\u{e9}\u{e8}\u{ea}\u{eb}\u{ed}\u{ec}\u{ee}\u{ef}\u{f1}\u{f3}\u{f2}\u{f4}\u{f6}\u{f5}\u{fa}\u{f9}\u{fb}\u{fc}\u{2020}\u{b0}\u{a2}\u{a3}\u{a7}\u{2022}\u{b6}\u{df}\u{ae}\u{a9}\u{2122}\u{b4}\u{a8}\u{2260}\u{c6}\u{d8}\u{221e}\u{b1}\u{2264}\u{2265}\u{a5}\u{b5}\u{2202}\u{2211}\u{220f}\u{3c0}\u{222b}\u{aa}\u{ba}\u{3a9}\u{e6}\u{f8}\u{bf}\u{a1}\u{ac}\u{221a}\u{192}\u{2248}\u{2206}\u{ab}\u{bb}\u{2026}\u{a0}\u{c0}\u{c3}\u{d5}\u{152}\u{153}\u{2013}\u{2014}\u{201c}\u{201d}\u{2018}\u{2019}\u{f7}\u{25ca}\u{ff}\u{178}\u{11e}\u{11f}\u{130}\u{131}\u{15e}\u{15f}\u{2021}\u{b7}\u{201a}\u{201e}\u{2030}\u{c2}\u{ca}\u{c1}\u{cb}\u{c8}\u{cd}\u{ce}\u{cf}\u{cc}\u{d3}\u{d4}\u{f8ff}\u{d2}\u{da}\u{db}\u{d9}\u{f8a0}\u{2c6}\u{2dc}\u{af}\u{2d8}\u{2d9}\u{2da}\u{b8}\u{2dd}\u{2db}\u{2c7}",
            undefined: "",
        },
    ),
    (
        "palmos",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{20ac}\u{81}\u{201a}\u{192}\u{201e}\u{2026}\u{2020}\u{2021}\u{2c6}\u{2030}\u{160}\u{2039}\u{152}\u{2666}\u{2663}\u{2665}\u{2660}\u{2018}\u{2019}\u{201c}\u{201d}\u{2022}\u{2013}\u{2014}\u{2dc}\u{2122}\u{161}\u{9b}\u{153}\u{9d}\u{9e}\u{178}\u{a0}\u{a1}\u{a2}\u{a3}\u{a4}\u{a5}\u{a6}\u{a7}\u{a8}\u{a9}\u{aa}\u{ab}\u{ac}\u{ad}\u{ae}\u{af}\u{b0}\u{b1}\u{b2}\u{b3}\u{b4}\u{b5}\u{b6}\u{b7}\u{b8}\u{b9}\u{ba}\u{bb}\u{bc}\u{bd}\u{be}\u{bf}\u{c0}\u{c1}\u{c2}\u{c3}\u{c4}\u{c5}\u{c6}\u{c7}\u{c8}\u{c9}\u{ca}\u{cb}\u{cc}\u{cd}\u{ce}\u{cf}\u{d0}\u{d1}\u{d2}\u{d3}\u{d4}\u{d5}\u{d6}\u{d7}\u{d8}\u{d9}\u{da}\u{db}\u{dc}\u{dd}\u{de}\u{df}\u{e0}\u{e1}\u{e2}\u{e3}\u{e4}\u{e5}\u{e6}\u{e7}\u{e8}\u{e9}\u{ea}\u{eb}\u{ec}\u{ed}\u{ee}\u{ef}\u{f0}\u{f1}\u{f2}\u{f3}\u{f4}\u{f5}\u{f6}\u{f7}\u{f8}\u{f9}\u{fa}\u{fb}\u{fc}\u{fd}\u{fe}\u{ff}",
            undefined: "",
        },
    ),
    (
        "ptcp154",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{496}\u{492}\u{4ee}\u{493}\u{201e}\u{2026}\u{4b6}\u{4ae}\u{4b2}\u{4af}\u{4a0}\u{4e2}\u{4a2}\u{49a}\u{4ba}\u{4b8}\u{497}\u{2018}\u{2019}\u{201c}\u{201d}\u{2022}\u{2013}\u{2014}\u{4b3}\u{4b7}\u{4a1}\u{4e3}\u{4a3}\u{49b}\u{4bb}\u{4b9}\u{a0}\u{40e}\u{45e}\u{408}\u{4e8}\u{498}\u{4b0}\u{a7}\u{401}\u{a9}\u{4d8}\u{ab}\u{ac}\u{4ef}\u{ae}\u{49c}\u{b0}\u{4b1}\u{406}\u{456}\u{499}\u{4e9}\u{b6}\u{b7}\u{451}\u{2116}\u{4d9}\u{bb}\u{458}\u{4aa}\u{4ab}\u{49d}\u{410}\u{411}\u{412}\u{413}\u{414}\u{415}\u{416}\u{417}\u{418}\u{419}\u{41a}\u{41b}\u{41c}\u{41d}\u{41e}\u{41f}\u{420}\u{421}\u{422}\u{423}\u{424}\u{425}\u{426}\u{427}\u{428}\u{429}\u{42a}\u{42b}\u{42c}\u{42d}\u{42e}\u{42f}\u{430}\u{431}\u{432}\u{433}\u{434}\u{435}\u{436}\u{437}\u{438}\u{439}\u{43a}\u{43b}\u{43c}\u{43d}\u{43e}\u{43f}\u{440}\u{441}\u{442}\u{443}\u{444}\u{445}\u{446}\u{447}\u{448}\u{449}\u{44a}\u{44b}\u{44c}\u{44d}\u{44e}\u{44f}",
            undefined: "",
        },
    ),
    (
        "raw-unicode-escape",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{a1}\u{a2}\u{a3}\u{a4}\u{a5}\u{a6}\u{a7}\u{a8}\u{a9}\u{aa}\u{ab}\u{ac}\u{ad}\u{ae}\u{af}\u{b0}\u{b1}\u{b2}\u{b3}\u{b4}\u{b5}\u{b6}\u{b7}\u{b8}\u{b9}\u{ba}\u{bb}\u{bc}\u{bd}\u{be}\u{bf}\u{c0}\u{c1}\u{c2}\u{c3}\u{c4}\u{c5}\u{c6}\u{c7}\u{c8}\u{c9}\u{ca}\u{cb}\u{cc}\u{cd}\u{ce}\u{cf}\u{d0}\u{d1}\u{d2}\u{d3}\u{d4}\u{d5}\u{d6}\u{d7}\u{d8}\u{d9}\u{da}\u{db}\u{dc}\u{dd}\u{de}\u{df}\u{e0}\u{e1}\u{e2}\u{e3}\u{e4}\u{e5}\u{e6}\u{e7}\u{e8}\u{e9}\u{ea}\u{eb}\u{ec}\u{ed}\u{ee}\u{ef}\u{f0}\u{f1}\u{f2}\u{f3}\u{f4}\u{f5}\u{f6}\u{f7}\u{f8}\u{f9}\u{fa}\u{fb}\u{fc}\u{fd}\u{fe}\u{ff}",
            undefined: "",
        },
    ),
    (
        "tis-620",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{fffd}\u{e01}\u{e02}\u{e03}\u{e04}\u{e05}\u{e06}\u{e07}\u{e08}\u{e09}\u{e0a}\u{e0b}\u{e0c}\u{e0d}\u{e0e}\u{e0f}\u{e10}\u{e11}\u{e12}\u{e13}\u{e14}\u{e15}\u{e16}\u{e17}\u{e18}\u{e19}\u{e1a}\u{e1b}\u{e1c}\u{e1d}\u{e1e}\u{e1f}\u{e20}\u{e21}\u{e22}\u{e23}\u{e24}\u{e25}\u{e26}\u{e27}\u{e28}\u{e29}\u{e2a}\u{e2b}\u{e2c}\u{e2d}\u{e2e}\u{e2f}\u{e30}\u{e31}\u{e32}\u{e33}\u{e34}\u{e35}\u{e36}\u{e37}\u{e38}\u{e39}\u{e3a}\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{e3f}\u{e40}\u{e41}\u{e42}\u{e43}\u{e44}\u{e45}\u{e46}\u{e47}\u{e48}\u{e49}\u{e4a}\u{e4b}\u{e4c}\u{e4d}\u{e4e}\u{e4f}\u{e50}\u{e51}\u{e52}\u{e53}\u{e54}\u{e55}\u{e56}\u{e57}\u{e58}\u{e59}\u{e5a}\u{e5b}\u{fffd}\u{fffd}\u{fffd}\u{fffd}",
            undefined: "\u{a0}\u{db}\u{dc}\u{dd}\u{de}\u{fc}\u{fd}\u{fe}\u{ff}",
        },
    ),
    (
        "unicode-escape",
        SingleByte {
            table: "\u{0}\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}\u{e}\u{f}\u{10}\u{11}\u{12}\u{13}\u{14}\u{15}\u{16}\u{17}\u{18}\u{19}\u{1a}\u{1b}\u{1c}\u{1d}\u{1e}\u{1f} !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\u{fffd}]^_`abcdefghijklmnopqrstuvwxyz{|}~\u{7f}\u{80}\u{81}\u{82}\u{83}\u{84}\u{85}\u{86}\u{87}\u{88}\u{89}\u{8a}\u{8b}\u{8c}\u{8d}\u{8e}\u{8f}\u{90}\u{91}\u{92}\u{93}\u{94}\u{95}\u{96}\u{97}\u{98}\u{99}\u{9a}\u{9b}\u{9c}\u{9d}\u{9e}\u{9f}\u{a0}\u{a1}\u{a2}\u{a3}\u{a4}\u{a5}\u{a6}\u{a7}\u{a8}\u{a9}\u{aa}\u{ab}\u{ac}\u{ad}\u{ae}\u{af}\u{b0}\u{b1}\u{b2}\u{b3}\u{b4}\u{b5}\u{b6}\u{b7}\u{b8}\u{b9}\u{ba}\u{bb}\u{bc}\u{bd}\u{be}\u{bf}\u{c0}\u{c1}\u{c2}\u{c3}\u{c4}\u{c5}\u{c6}\u{c7}\u{c8}\u{c9}\u{ca}\u{cb}\u{cc}\u{cd}\u{ce}\u{cf}\u{d0}\u{d1}\u{d2}\u{d3}\u{d4}\u{d5}\u{d6}\u{d7}\u{d8}\u{d9}\u{da}\u{db}\u{dc}\u{dd}\u{de}\u{df}\u{e0}\u{e1}\u{e2}\u{e3}\u{e4}\u{e5}\u{e6}\u{e7}\u{e8}\u{e9}\u{ea}\u{eb}\u{ec}\u{ed}\u{ee}\u{ef}\u{f0}\u{f1}\u{f2}\u{f3}\u{f4}\u{f5}\u{f6}\u{f7}\u{f8}\u{f9}\u{fa}\u{fb}\u{fc}\u{fd}\u{fe}\u{ff}",
            undefined: "\\",
        },
    ),
];

/// The rule each label decodes by, keyed by Python's normalized spelling.
static RULES: LazyLock<HashMap<&'static str, Rule>> = LazyLock::new(|| {
    let mut rules = HashMap::new();
    rules.insert("037", Rule::Refused);
    rules.insert("1026", Rule::Refused);
    rules.insert("1125", Rule::Single(table("cp1125")));
    rules.insert("1140", Rule::Refused);
    rules.insert("1250", Rule::Single(table("cp1250")));
    rules.insert("1251", Rule::Single(table("cp1251")));
    rules.insert("1252", Rule::Single(table("cp1252")));
    rules.insert("1253", Rule::Single(table("cp1253")));
    rules.insert("1254", Rule::Single(table("cp1254")));
    rules.insert("1255", Rule::Single(table("cp1255")));
    rules.insert("1256", Rule::Single(table("cp1256")));
    rules.insert("1257", Rule::Single(table("cp1257")));
    rules.insert("1258", Rule::Single(table("cp1258")));
    rules.insert("273", Rule::Refused);
    rules.insert("424", Rule::Refused);
    rules.insert("437", Rule::Single(table("cp437")));
    rules.insert("500", Rule::Refused);
    rules.insert("646", Rule::Single(table("ascii")));
    rules.insert("775", Rule::Single(table("cp775")));
    rules.insert("850", Rule::Single(table("cp850")));
    rules.insert("852", Rule::Single(table("cp852")));
    rules.insert("855", Rule::Single(table("cp855")));
    rules.insert("857", Rule::Single(table("cp857")));
    rules.insert("858", Rule::Single(table("cp858")));
    rules.insert("860", Rule::Single(table("cp860")));
    rules.insert("861", Rule::Single(table("cp861")));
    rules.insert("862", Rule::Single(table("cp862")));
    rules.insert("863", Rule::Single(table("cp863")));
    rules.insert("864", Rule::Lossy);
    rules.insert("865", Rule::Single(table("cp865")));
    rules.insert("866", Rule::Single(table("cp866")));
    rules.insert("869", Rule::Single(table("cp869")));
    rules.insert("8859", Rule::Single(table("iso8859-1")));
    rules.insert("932", Rule::Refused);
    rules.insert("936", Rule::Refused);
    rules.insert("949", Rule::Refused);
    rules.insert("950", Rule::Refused);
    rules.insert("ansi_x3_4_1968", Rule::Single(table("ascii")));
    rules.insert("ansi_x3_4_1986", Rule::Single(table("ascii")));
    rules.insert("arabic", Rule::Single(table("iso8859-6")));
    rules.insert("ascii", Rule::Single(table("ascii")));
    rules.insert("asmo_708", Rule::Single(table("iso8859-6")));
    rules.insert("big5", Rule::Refused);
    rules.insert("big5_hkscs", Rule::Refused);
    rules.insert("big5_tw", Rule::Refused);
    rules.insert("big5hkscs", Rule::Refused);
    rules.insert("charmap", Rule::Single(table("charmap")));
    rules.insert("chinese", Rule::Refused);
    rules.insert("cp037", Rule::Refused);
    rules.insert("cp1006", Rule::Single(table("cp1006")));
    rules.insert("cp1026", Rule::Refused);
    rules.insert("cp1051", Rule::Single(table("hp-roman8")));
    rules.insert("cp1125", Rule::Single(table("cp1125")));
    rules.insert("cp1140", Rule::Refused);
    rules.insert("cp1250", Rule::Single(table("cp1250")));
    rules.insert("cp1251", Rule::Single(table("cp1251")));
    rules.insert("cp1252", Rule::Single(table("cp1252")));
    rules.insert("cp1253", Rule::Single(table("cp1253")));
    rules.insert("cp1254", Rule::Single(table("cp1254")));
    rules.insert("cp1255", Rule::Single(table("cp1255")));
    rules.insert("cp1256", Rule::Single(table("cp1256")));
    rules.insert("cp1257", Rule::Single(table("cp1257")));
    rules.insert("cp1258", Rule::Single(table("cp1258")));
    rules.insert("cp1361", Rule::Refused);
    rules.insert("cp154", Rule::Single(table("ptcp154")));
    rules.insert("cp273", Rule::Refused);
    rules.insert("cp367", Rule::Single(table("ascii")));
    rules.insert("cp424", Rule::Refused);
    rules.insert("cp437", Rule::Single(table("cp437")));
    rules.insert("cp500", Rule::Refused);
    rules.insert("cp65001", Rule::Utf8);
    rules.insert("cp720", Rule::Single(table("cp720")));
    rules.insert("cp737", Rule::Single(table("cp737")));
    rules.insert("cp775", Rule::Single(table("cp775")));
    rules.insert("cp819", Rule::Single(table("iso8859-1")));
    rules.insert("cp850", Rule::Single(table("cp850")));
    rules.insert("cp852", Rule::Single(table("cp852")));
    rules.insert("cp855", Rule::Single(table("cp855")));
    rules.insert("cp856", Rule::Single(table("cp856")));
    rules.insert("cp857", Rule::Single(table("cp857")));
    rules.insert("cp858", Rule::Single(table("cp858")));
    rules.insert("cp860", Rule::Single(table("cp860")));
    rules.insert("cp861", Rule::Single(table("cp861")));
    rules.insert("cp862", Rule::Single(table("cp862")));
    rules.insert("cp863", Rule::Single(table("cp863")));
    rules.insert("cp864", Rule::Lossy);
    rules.insert("cp865", Rule::Single(table("cp865")));
    rules.insert("cp866", Rule::Single(table("cp866")));
    rules.insert("cp866u", Rule::Single(table("cp1125")));
    rules.insert("cp869", Rule::Single(table("cp869")));
    rules.insert("cp874", Rule::Single(table("cp874")));
    rules.insert("cp875", Rule::Refused);
    rules.insert("cp932", Rule::Refused);
    rules.insert("cp936", Rule::Refused);
    rules.insert("cp949", Rule::Refused);
    rules.insert("cp950", Rule::Refused);
    rules.insert("cp_gr", Rule::Single(table("cp869")));
    rules.insert("cp_is", Rule::Single(table("cp861")));
    rules.insert("csascii", Rule::Single(table("ascii")));
    rules.insert("csbig5", Rule::Refused);
    rules.insert("csibm037", Rule::Refused);
    rules.insert("csibm1026", Rule::Refused);
    rules.insert("csibm273", Rule::Refused);
    rules.insert("csibm424", Rule::Refused);
    rules.insert("csibm500", Rule::Refused);
    rules.insert("csibm855", Rule::Single(table("cp855")));
    rules.insert("csibm857", Rule::Single(table("cp857")));
    rules.insert("csibm858", Rule::Single(table("cp858")));
    rules.insert("csibm860", Rule::Single(table("cp860")));
    rules.insert("csibm861", Rule::Single(table("cp861")));
    rules.insert("csibm863", Rule::Single(table("cp863")));
    rules.insert("csibm864", Rule::Lossy);
    rules.insert("csibm865", Rule::Single(table("cp865")));
    rules.insert("csibm866", Rule::Single(table("cp866")));
    rules.insert("csibm869", Rule::Single(table("cp869")));
    rules.insert("csiso2022jp", Rule::Refused);
    rules.insert("csiso2022kr", Rule::Refused);
    rules.insert("csiso58gb231280", Rule::Refused);
    rules.insert("csisolatin1", Rule::Single(table("iso8859-1")));
    rules.insert("csisolatin2", Rule::Single(table("iso8859-2")));
    rules.insert("csisolatin3", Rule::Single(table("iso8859-3")));
    rules.insert("csisolatin4", Rule::Single(table("iso8859-4")));
    rules.insert("csisolatin5", Rule::Single(table("iso8859-9")));
    rules.insert("csisolatin6", Rule::Single(table("iso8859-10")));
    rules.insert("csisolatinarabic", Rule::Single(table("iso8859-6")));
    rules.insert("csisolatincyrillic", Rule::Single(table("iso8859-5")));
    rules.insert("csisolatingreek", Rule::Single(table("iso8859-7")));
    rules.insert("csisolatinhebrew", Rule::Single(table("iso8859-8")));
    rules.insert("cskoi8r", Rule::Single(table("koi8-r")));
    rules.insert("cspc775baltic", Rule::Single(table("cp775")));
    rules.insert("cspc850multilingual", Rule::Single(table("cp850")));
    rules.insert("cspc862latinhebrew", Rule::Single(table("cp862")));
    rules.insert("cspc8codepage437", Rule::Single(table("cp437")));
    rules.insert("cspcp852", Rule::Single(table("cp852")));
    rules.insert("csptcp154", Rule::Single(table("ptcp154")));
    rules.insert("csshiftjis", Rule::Refused);
    rules.insert("cyrillic", Rule::Single(table("iso8859-5")));
    rules.insert("cyrillic_asian", Rule::Single(table("ptcp154")));
    rules.insert("ebcdic_cp_be", Rule::Refused);
    rules.insert("ebcdic_cp_ca", Rule::Refused);
    rules.insert("ebcdic_cp_ch", Rule::Refused);
    rules.insert("ebcdic_cp_he", Rule::Refused);
    rules.insert("ebcdic_cp_nl", Rule::Refused);
    rules.insert("ebcdic_cp_us", Rule::Refused);
    rules.insert("ebcdic_cp_wt", Rule::Refused);
    rules.insert("ecma_114", Rule::Single(table("iso8859-6")));
    rules.insert("ecma_118", Rule::Single(table("iso8859-7")));
    rules.insert("elot_928", Rule::Single(table("iso8859-7")));
    rules.insert("euc_cn", Rule::Refused);
    rules.insert("euc_jis2004", Rule::Refused);
    rules.insert("euc_jis_2004", Rule::Refused);
    rules.insert("euc_jisx0213", Rule::Refused);
    rules.insert("euc_jp", Rule::Refused);
    rules.insert("euc_kr", Rule::Refused);
    rules.insert("euccn", Rule::Refused);
    rules.insert("eucgb2312_cn", Rule::Refused);
    rules.insert("eucjis2004", Rule::Refused);
    rules.insert("eucjisx0213", Rule::Refused);
    rules.insert("eucjp", Rule::Refused);
    rules.insert("euckr", Rule::Refused);
    rules.insert("gb18030", Rule::Refused);
    rules.insert("gb18030_2000", Rule::Refused);
    rules.insert("gb2312", Rule::Refused);
    rules.insert("gb2312_1980", Rule::Refused);
    rules.insert("gb2312_80", Rule::Refused);
    rules.insert("gbk", Rule::Refused);
    rules.insert("greek", Rule::Single(table("iso8859-7")));
    rules.insert("greek8", Rule::Single(table("iso8859-7")));
    rules.insert("hebrew", Rule::Single(table("iso8859-8")));
    rules.insert("hkscs", Rule::Refused);
    rules.insert("hp_roman8", Rule::Single(table("hp-roman8")));
    rules.insert("hz", Rule::Refused);
    rules.insert("hz_gb", Rule::Refused);
    rules.insert("hz_gb_2312", Rule::Refused);
    rules.insert("hzgb", Rule::Refused);
    rules.insert("ibm037", Rule::Refused);
    rules.insert("ibm039", Rule::Refused);
    rules.insert("ibm1026", Rule::Refused);
    rules.insert("ibm1051", Rule::Single(table("hp-roman8")));
    rules.insert("ibm1125", Rule::Single(table("cp1125")));
    rules.insert("ibm1140", Rule::Refused);
    rules.insert("ibm273", Rule::Refused);
    rules.insert("ibm367", Rule::Single(table("ascii")));
    rules.insert("ibm424", Rule::Refused);
    rules.insert("ibm437", Rule::Single(table("cp437")));
    rules.insert("ibm500", Rule::Refused);
    rules.insert("ibm775", Rule::Single(table("cp775")));
    rules.insert("ibm819", Rule::Single(table("iso8859-1")));
    rules.insert("ibm850", Rule::Single(table("cp850")));
    rules.insert("ibm852", Rule::Single(table("cp852")));
    rules.insert("ibm855", Rule::Single(table("cp855")));
    rules.insert("ibm857", Rule::Single(table("cp857")));
    rules.insert("ibm858", Rule::Single(table("cp858")));
    rules.insert("ibm860", Rule::Single(table("cp860")));
    rules.insert("ibm861", Rule::Single(table("cp861")));
    rules.insert("ibm862", Rule::Single(table("cp862")));
    rules.insert("ibm863", Rule::Single(table("cp863")));
    rules.insert("ibm864", Rule::Lossy);
    rules.insert("ibm865", Rule::Single(table("cp865")));
    rules.insert("ibm866", Rule::Single(table("cp866")));
    rules.insert("ibm869", Rule::Single(table("cp869")));
    rules.insert("idna", Rule::Refused);
    rules.insert("iso2022_jp", Rule::Refused);
    rules.insert("iso2022_jp_1", Rule::Refused);
    rules.insert("iso2022_jp_2", Rule::Refused);
    rules.insert("iso2022_jp_2004", Rule::Refused);
    rules.insert("iso2022_jp_3", Rule::Refused);
    rules.insert("iso2022_jp_ext", Rule::Refused);
    rules.insert("iso2022_kr", Rule::Refused);
    rules.insert("iso2022jp", Rule::Refused);
    rules.insert("iso2022jp_1", Rule::Refused);
    rules.insert("iso2022jp_2", Rule::Refused);
    rules.insert("iso2022jp_2004", Rule::Refused);
    rules.insert("iso2022jp_3", Rule::Refused);
    rules.insert("iso2022jp_ext", Rule::Refused);
    rules.insert("iso2022kr", Rule::Refused);
    rules.insert("iso646_us", Rule::Single(table("ascii")));
    rules.insert("iso8859", Rule::Single(table("iso8859-1")));
    rules.insert("iso8859_1", Rule::Single(table("iso8859-1")));
    rules.insert("iso8859_10", Rule::Single(table("iso8859-10")));
    rules.insert("iso8859_11", Rule::Single(table("iso8859-11")));
    rules.insert("iso8859_13", Rule::Single(table("iso8859-13")));
    rules.insert("iso8859_14", Rule::Single(table("iso8859-14")));
    rules.insert("iso8859_15", Rule::Single(table("iso8859-15")));
    rules.insert("iso8859_16", Rule::Single(table("iso8859-16")));
    rules.insert("iso8859_2", Rule::Single(table("iso8859-2")));
    rules.insert("iso8859_3", Rule::Single(table("iso8859-3")));
    rules.insert("iso8859_4", Rule::Single(table("iso8859-4")));
    rules.insert("iso8859_5", Rule::Single(table("iso8859-5")));
    rules.insert("iso8859_6", Rule::Single(table("iso8859-6")));
    rules.insert("iso8859_7", Rule::Single(table("iso8859-7")));
    rules.insert("iso8859_8", Rule::Single(table("iso8859-8")));
    rules.insert("iso8859_9", Rule::Single(table("iso8859-9")));
    rules.insert("iso_2022_jp", Rule::Refused);
    rules.insert("iso_2022_jp_1", Rule::Refused);
    rules.insert("iso_2022_jp_2", Rule::Refused);
    rules.insert("iso_2022_jp_2004", Rule::Refused);
    rules.insert("iso_2022_jp_3", Rule::Refused);
    rules.insert("iso_2022_jp_ext", Rule::Refused);
    rules.insert("iso_2022_kr", Rule::Refused);
    rules.insert("iso_646_irv_1991", Rule::Single(table("ascii")));
    rules.insert("iso_8859_1", Rule::Single(table("iso8859-1")));
    rules.insert("iso_8859_10", Rule::Single(table("iso8859-10")));
    rules.insert("iso_8859_10_1992", Rule::Single(table("iso8859-10")));
    rules.insert("iso_8859_11", Rule::Single(table("iso8859-11")));
    rules.insert("iso_8859_11_2001", Rule::Single(table("iso8859-11")));
    rules.insert("iso_8859_13", Rule::Single(table("iso8859-13")));
    rules.insert("iso_8859_14", Rule::Single(table("iso8859-14")));
    rules.insert("iso_8859_14_1998", Rule::Single(table("iso8859-14")));
    rules.insert("iso_8859_15", Rule::Single(table("iso8859-15")));
    rules.insert("iso_8859_16", Rule::Single(table("iso8859-16")));
    rules.insert("iso_8859_16_2001", Rule::Single(table("iso8859-16")));
    rules.insert("iso_8859_1_1987", Rule::Single(table("iso8859-1")));
    rules.insert("iso_8859_2", Rule::Single(table("iso8859-2")));
    rules.insert("iso_8859_2_1987", Rule::Single(table("iso8859-2")));
    rules.insert("iso_8859_3", Rule::Single(table("iso8859-3")));
    rules.insert("iso_8859_3_1988", Rule::Single(table("iso8859-3")));
    rules.insert("iso_8859_4", Rule::Single(table("iso8859-4")));
    rules.insert("iso_8859_4_1988", Rule::Single(table("iso8859-4")));
    rules.insert("iso_8859_5", Rule::Single(table("iso8859-5")));
    rules.insert("iso_8859_5_1988", Rule::Single(table("iso8859-5")));
    rules.insert("iso_8859_6", Rule::Single(table("iso8859-6")));
    rules.insert("iso_8859_6_1987", Rule::Single(table("iso8859-6")));
    rules.insert("iso_8859_7", Rule::Single(table("iso8859-7")));
    rules.insert("iso_8859_7_1987", Rule::Single(table("iso8859-7")));
    rules.insert("iso_8859_8", Rule::Single(table("iso8859-8")));
    rules.insert("iso_8859_8_1988", Rule::Single(table("iso8859-8")));
    rules.insert("iso_8859_9", Rule::Single(table("iso8859-9")));
    rules.insert("iso_8859_9_1989", Rule::Single(table("iso8859-9")));
    rules.insert("iso_celtic", Rule::Single(table("iso8859-14")));
    rules.insert("iso_ir_100", Rule::Single(table("iso8859-1")));
    rules.insert("iso_ir_101", Rule::Single(table("iso8859-2")));
    rules.insert("iso_ir_109", Rule::Single(table("iso8859-3")));
    rules.insert("iso_ir_110", Rule::Single(table("iso8859-4")));
    rules.insert("iso_ir_126", Rule::Single(table("iso8859-7")));
    rules.insert("iso_ir_127", Rule::Single(table("iso8859-6")));
    rules.insert("iso_ir_138", Rule::Single(table("iso8859-8")));
    rules.insert("iso_ir_144", Rule::Single(table("iso8859-5")));
    rules.insert("iso_ir_148", Rule::Single(table("iso8859-9")));
    rules.insert("iso_ir_157", Rule::Single(table("iso8859-10")));
    rules.insert("iso_ir_166", Rule::Single(table("tis-620")));
    rules.insert("iso_ir_199", Rule::Single(table("iso8859-14")));
    rules.insert("iso_ir_226", Rule::Single(table("iso8859-16")));
    rules.insert("iso_ir_58", Rule::Refused);
    rules.insert("iso_ir_6", Rule::Single(table("ascii")));
    rules.insert("jisx0213", Rule::Refused);
    rules.insert("johab", Rule::Refused);
    rules.insert("koi8_r", Rule::Single(table("koi8-r")));
    rules.insert("koi8_t", Rule::Single(table("koi8-t")));
    rules.insert("koi8_u", Rule::Single(table("koi8-u")));
    rules.insert("korean", Rule::Refused);
    rules.insert("ks_c_5601", Rule::Refused);
    rules.insert("ks_c_5601_1987", Rule::Refused);
    rules.insert("ks_x_1001", Rule::Refused);
    rules.insert("ksc5601", Rule::Refused);
    rules.insert("ksx1001", Rule::Refused);
    rules.insert("kz1048", Rule::Single(table("kz1048")));
    rules.insert("kz_1048", Rule::Single(table("kz1048")));
    rules.insert("l1", Rule::Single(table("iso8859-1")));
    rules.insert("l10", Rule::Single(table("iso8859-16")));
    rules.insert("l2", Rule::Single(table("iso8859-2")));
    rules.insert("l3", Rule::Single(table("iso8859-3")));
    rules.insert("l4", Rule::Single(table("iso8859-4")));
    rules.insert("l5", Rule::Single(table("iso8859-9")));
    rules.insert("l6", Rule::Single(table("iso8859-10")));
    rules.insert("l7", Rule::Single(table("iso8859-13")));
    rules.insert("l8", Rule::Single(table("iso8859-14")));
    rules.insert("l9", Rule::Single(table("iso8859-15")));
    rules.insert("latin", Rule::Single(table("iso8859-1")));
    rules.insert("latin1", Rule::Single(table("iso8859-1")));
    rules.insert("latin10", Rule::Single(table("iso8859-16")));
    rules.insert("latin2", Rule::Single(table("iso8859-2")));
    rules.insert("latin3", Rule::Single(table("iso8859-3")));
    rules.insert("latin4", Rule::Single(table("iso8859-4")));
    rules.insert("latin5", Rule::Single(table("iso8859-9")));
    rules.insert("latin6", Rule::Single(table("iso8859-10")));
    rules.insert("latin7", Rule::Single(table("iso8859-13")));
    rules.insert("latin8", Rule::Single(table("iso8859-14")));
    rules.insert("latin9", Rule::Single(table("iso8859-15")));
    rules.insert("latin_1", Rule::Single(table("iso8859-1")));
    rules.insert("mac_arabic", Rule::Single(table("mac-arabic")));
    rules.insert("mac_centeuro", Rule::Single(table("mac-latin2")));
    rules.insert("mac_croatian", Rule::Single(table("mac-croatian")));
    rules.insert("mac_cyrillic", Rule::Single(table("mac-cyrillic")));
    rules.insert("mac_farsi", Rule::Single(table("mac-farsi")));
    rules.insert("mac_greek", Rule::Single(table("mac-greek")));
    rules.insert("mac_iceland", Rule::Single(table("mac-iceland")));
    rules.insert("mac_latin2", Rule::Single(table("mac-latin2")));
    rules.insert("mac_roman", Rule::Single(table("mac-roman")));
    rules.insert("mac_romanian", Rule::Single(table("mac-romanian")));
    rules.insert("mac_turkish", Rule::Single(table("mac-turkish")));
    rules.insert("maccentraleurope", Rule::Single(table("mac-latin2")));
    rules.insert("maccyrillic", Rule::Single(table("mac-cyrillic")));
    rules.insert("macgreek", Rule::Single(table("mac-greek")));
    rules.insert("maciceland", Rule::Single(table("mac-iceland")));
    rules.insert("macintosh", Rule::Single(table("mac-roman")));
    rules.insert("maclatin2", Rule::Single(table("mac-latin2")));
    rules.insert("macroman", Rule::Single(table("mac-roman")));
    rules.insert("macturkish", Rule::Single(table("mac-turkish")));
    rules.insert("ms1361", Rule::Refused);
    rules.insert("ms932", Rule::Refused);
    rules.insert("ms936", Rule::Refused);
    rules.insert("ms949", Rule::Refused);
    rules.insert("ms950", Rule::Refused);
    rules.insert("ms_kanji", Rule::Refused);
    rules.insert("mskanji", Rule::Refused);
    rules.insert("palmos", Rule::Single(table("palmos")));
    rules.insert("pt154", Rule::Single(table("ptcp154")));
    rules.insert("ptcp154", Rule::Single(table("ptcp154")));
    rules.insert("punycode", Rule::Refused);
    rules.insert("r8", Rule::Single(table("hp-roman8")));
    rules.insert(
        "raw_unicode_escape",
        Rule::Single(table("raw-unicode-escape")),
    );
    rules.insert("rk1048", Rule::Single(table("kz1048")));
    rules.insert("roman8", Rule::Single(table("hp-roman8")));
    rules.insert("ruscii", Rule::Single(table("cp1125")));
    rules.insert("s_jis", Rule::Refused);
    rules.insert("s_jis_2004", Rule::Refused);
    rules.insert("s_jisx0213", Rule::Refused);
    rules.insert("shift_jis", Rule::Refused);
    rules.insert("shift_jis_2004", Rule::Refused);
    rules.insert("shift_jisx0213", Rule::Refused);
    rules.insert("shiftjis", Rule::Refused);
    rules.insert("shiftjis2004", Rule::Refused);
    rules.insert("shiftjisx0213", Rule::Refused);
    rules.insert("sjis", Rule::Refused);
    rules.insert("sjis_2004", Rule::Refused);
    rules.insert("sjisx0213", Rule::Refused);
    rules.insert("strk1048_2002", Rule::Single(table("kz1048")));
    rules.insert("thai", Rule::Single(table("iso8859-11")));
    rules.insert("tis620", Rule::Single(table("tis-620")));
    rules.insert("tis_620", Rule::Single(table("tis-620")));
    rules.insert("tis_620_0", Rule::Single(table("tis-620")));
    rules.insert("tis_620_2529_0", Rule::Single(table("tis-620")));
    rules.insert("tis_620_2529_1", Rule::Single(table("tis-620")));
    rules.insert("u16", Rule::Refused);
    rules.insert("u32", Rule::Refused);
    rules.insert("u7", Rule::Refused);
    rules.insert("u8", Rule::Utf8);
    rules.insert("u_jis", Rule::Refused);
    rules.insert("uhc", Rule::Refused);
    rules.insert("ujis", Rule::Refused);
    rules.insert("undefined", Rule::Refused);
    rules.insert("unicode_1_1_utf_7", Rule::Refused);
    rules.insert("unicode_escape", Rule::Single(table("unicode-escape")));
    rules.insert("unicodebigunmarked", Rule::Refused);
    rules.insert("unicodelittleunmarked", Rule::Refused);
    rules.insert("us", Rule::Single(table("ascii")));
    rules.insert("us_ascii", Rule::Single(table("ascii")));
    rules.insert("utf", Rule::Utf8);
    rules.insert("utf16", Rule::Refused);
    rules.insert("utf32", Rule::Refused);
    rules.insert("utf7", Rule::Refused);
    rules.insert("utf8", Rule::Utf8);
    rules.insert("utf8_ucs2", Rule::Utf8);
    rules.insert("utf8_ucs4", Rule::Utf8);
    rules.insert("utf_16", Rule::Refused);
    rules.insert("utf_16_be", Rule::Refused);
    rules.insert("utf_16_le", Rule::Refused);
    rules.insert("utf_16be", Rule::Refused);
    rules.insert("utf_16le", Rule::Refused);
    rules.insert("utf_32", Rule::Refused);
    rules.insert("utf_32_be", Rule::Refused);
    rules.insert("utf_32_le", Rule::Refused);
    rules.insert("utf_32be", Rule::Refused);
    rules.insert("utf_32le", Rule::Refused);
    rules.insert("utf_7", Rule::Refused);
    rules.insert("utf_8", Rule::Utf8);
    rules.insert("utf_8_sig", Rule::Utf8);
    rules.insert("windows_1250", Rule::Single(table("cp1250")));
    rules.insert("windows_1251", Rule::Single(table("cp1251")));
    rules.insert("windows_1252", Rule::Single(table("cp1252")));
    rules.insert("windows_1253", Rule::Single(table("cp1253")));
    rules.insert("windows_1254", Rule::Single(table("cp1254")));
    rules.insert("windows_1255", Rule::Single(table("cp1255")));
    rules.insert("windows_1256", Rule::Single(table("cp1256")));
    rules.insert("windows_1257", Rule::Single(table("cp1257")));
    rules.insert("windows_1258", Rule::Single(table("cp1258")));
    rules.insert("x_mac_japanese", Rule::Refused);
    rules.insert("x_mac_korean", Rule::Refused);
    rules.insert("x_mac_simp_chinese", Rule::Refused);
    rules.insert("x_mac_trad_chinese", Rule::Refused);
    rules
});

/// The rule `label` decodes by, when `CPython`'s codecs know that label.
pub(crate) fn rule_for(label: &str) -> Option<Rule> {
    RULES.get(normalized(label).as_str()).copied()
}

/// The code page `name` holds, which the rule table has already named.
#[expect(
    clippy::panic,
    reason = "every code page the rule table names is in this file, so a missing \
              one is a programming error that must stop the process"
)]
fn table(name: &str) -> SingleByte {
    TABLES
        .iter()
        .find_map(|(codec, table)| (*codec == name).then_some(*table))
        .unwrap_or_else(|| panic!("the {name} code page has no table"))
}

/// Python's label normalization: lowercase, and every run of non-alphanumeric
/// characters becomes one `_`.
fn normalized(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut underscore = false;
    for char in label.chars() {
        if char.is_alphanumeric() {
            if underscore && !out.is_empty() {
                out.push('_');
            }
            underscore = false;
            for lowered in char.to_lowercase() {
                out.push(lowered);
            }
        } else {
            underscore = true;
        }
    }
    out
}
