//! Known libc / MSVC CRT function signatures.
//!
//! The codegen C backend uses this table to:
//!  - Emit the correct return type instead of `uint64_t`.
//!  - Emit named parameter types in call expressions.
//!  - Detect variadic functions so excess args are kept.
//!
//! The table is intentionally small — it covers the ~40 most common symbols
//! seen in compiled C/C++ binaries.  Unknown functions fall back to the
//! generic `uint64_t func(...)` signature already emitted by the backend.

use std::collections::HashMap;
use std::sync::OnceLock;

use rustdec_ir::IrType;

// ── Public types ──────────────────────────────────────────────────────────────

/// Parameter descriptor — name is advisory only (used for comments / docs).
#[derive(Debug, Clone)]
pub struct Param {
    pub name: &'static str,
    pub ty:   IrType,
}

impl Param {
    const fn new(name: &'static str, ty: IrType) -> Self {
        Self { name, ty }
    }
}

/// Signature for a known external function.
#[derive(Debug, Clone)]
pub struct FunctionSig {
    /// Return type.
    pub ret:      IrType,
    /// Fixed parameters (may be empty for `void` or unknown-param functions).
    pub params:   &'static [Param],
    /// `true` for functions like `printf` that accept extra arguments.
    pub variadic: bool,
}

// ── Lookup ────────────────────────────────────────────────────────────────────

/// Return the known signature for `name`, or `None` if it is not in the table.
pub fn lookup(name: &str) -> Option<&'static FunctionSig> {
    TABLE.get_or_init(build_table).get(name)
}

// ── Table construction ────────────────────────────────────────────────────────

static TABLE: OnceLock<HashMap<&'static str, FunctionSig>> = OnceLock::new();

macro_rules! p {
    ($name:expr, $ty:expr) => {
        Param::new($name, $ty)
    };
}

fn build_table() -> HashMap<&'static str, FunctionSig> {
    use IrType::*;

    // Shorthands for common types.
    let _i8p = || Ptr(Box::new(SInt(8))); // reserved — int8_t* not yet used below
    let u8p  = || Ptr(Box::new(UInt(8)));
    let vp   = || Ptr(Box::new(Void));
    let i32_ = || SInt(32);
    let i64_ = || SInt(64);
    let u64_ = || UInt(64);
    let sz   = || UInt(64); // size_t

    // Leak slices into 'static so the FunctionSig can hold &'static [Param].
    // This is fine because the table lives for the entire program lifetime.
    macro_rules! params {
        ($($p:expr),* $(,)?) => {{
            let v: Vec<Param> = vec![$($p),*];
            Box::leak(v.into_boxed_slice()) as &'static [Param]
        }};
    }

    macro_rules! sig {
        (ret: $ret:expr, params: [$($p:expr),* $(,)?] $(, variadic: $va:expr)?) => {
            FunctionSig {
                ret:      $ret,
                params:   params!($($p),*),
                variadic: false $( || $va )?,
            }
        };
    }

    let mut m: HashMap<&'static str, FunctionSig> = HashMap::new();

    // ── stdio ─────────────────────────────────────────────────────────────────
    m.insert("printf",   sig!(ret: i32_(), params: [p!("fmt", u8p())], variadic: true));
    m.insert("fprintf",  sig!(ret: i32_(), params: [p!("stream", vp()), p!("fmt", u8p())], variadic: true));
    m.insert("sprintf",  sig!(ret: i32_(), params: [p!("buf", u8p()), p!("fmt", u8p())], variadic: true));
    m.insert("snprintf", sig!(ret: i32_(), params: [p!("buf", u8p()), p!("n", sz()), p!("fmt", u8p())], variadic: true));
    m.insert("puts",     sig!(ret: i32_(), params: [p!("s", u8p())]));
    m.insert("fputs",    sig!(ret: i32_(), params: [p!("s", u8p()), p!("stream", vp())]));
    m.insert("fputc",    sig!(ret: i32_(), params: [p!("c", i32_()), p!("stream", vp())]));
    m.insert("putchar",  sig!(ret: i32_(), params: [p!("c", i32_())]));
    m.insert("getchar",  sig!(ret: i32_(), params: []));
    m.insert("fgets",    sig!(ret: u8p(), params: [p!("buf", u8p()), p!("n", i32_()), p!("stream", vp())]));
    m.insert("fopen",    sig!(ret: vp(), params: [p!("path", u8p()), p!("mode", u8p())]));
    m.insert("fclose",   sig!(ret: i32_(), params: [p!("stream", vp())]));
    m.insert("fread",    sig!(ret: sz(), params: [p!("buf", vp()), p!("size", sz()), p!("n", sz()), p!("stream", vp())]));
    m.insert("fwrite",   sig!(ret: sz(), params: [p!("buf", vp()), p!("size", sz()), p!("n", sz()), p!("stream", vp())]));
    m.insert("fseek",    sig!(ret: i32_(), params: [p!("stream", vp()), p!("offset", i64_()), p!("whence", i32_())]));
    m.insert("ftell",    sig!(ret: i64_(), params: [p!("stream", vp())]));
    m.insert("rewind",   sig!(ret: Void, params: [p!("stream", vp())]));
    m.insert("fflush",   sig!(ret: i32_(), params: [p!("stream", vp())]));
    m.insert("scanf",    sig!(ret: i32_(), params: [p!("fmt", u8p())], variadic: true));
    m.insert("sscanf",   sig!(ret: i32_(), params: [p!("buf", u8p()), p!("fmt", u8p())], variadic: true));

    // ── stdlib ────────────────────────────────────────────────────────────────
    m.insert("malloc",   sig!(ret: vp(), params: [p!("size", sz())]));
    m.insert("calloc",   sig!(ret: vp(), params: [p!("n", sz()), p!("size", sz())]));
    m.insert("realloc",  sig!(ret: vp(), params: [p!("ptr", vp()), p!("size", sz())]));
    m.insert("free",     sig!(ret: Void, params: [p!("ptr", vp())]));
    m.insert("exit",     sig!(ret: Void, params: [p!("status", i32_())]));
    m.insert("abort",    sig!(ret: Void, params: []));
    m.insert("atoi",     sig!(ret: i32_(), params: [p!("s", u8p())]));
    m.insert("atol",     sig!(ret: i64_(), params: [p!("s", u8p())]));
    m.insert("atof",     sig!(ret: Float(64), params: [p!("s", u8p())]));
    m.insert("strtol",   sig!(ret: i64_(), params: [p!("s", u8p()), p!("end", Ptr(Box::new(u8p()))), p!("base", i32_())]));
    m.insert("strtoul",  sig!(ret: u64_(), params: [p!("s", u8p()), p!("end", Ptr(Box::new(u8p()))), p!("base", i32_())]));
    m.insert("qsort",    sig!(ret: Void, params: [p!("base", vp()), p!("n", sz()), p!("size", sz()), p!("cmp", vp())]));

    // ── string.h ──────────────────────────────────────────────────────────────
    m.insert("strlen",   sig!(ret: sz(), params: [p!("s", u8p())]));
    m.insert("strcpy",   sig!(ret: u8p(), params: [p!("dst", u8p()), p!("src", u8p())]));
    m.insert("strncpy",  sig!(ret: u8p(), params: [p!("dst", u8p()), p!("src", u8p()), p!("n", sz())]));
    m.insert("strcat",   sig!(ret: u8p(), params: [p!("dst", u8p()), p!("src", u8p())]));
    m.insert("strncat",  sig!(ret: u8p(), params: [p!("dst", u8p()), p!("src", u8p()), p!("n", sz())]));
    m.insert("strcmp",   sig!(ret: i32_(), params: [p!("a", u8p()), p!("b", u8p())]));
    m.insert("strncmp",  sig!(ret: i32_(), params: [p!("a", u8p()), p!("b", u8p()), p!("n", sz())]));
    m.insert("strchr",   sig!(ret: u8p(), params: [p!("s", u8p()), p!("c", i32_())]));
    m.insert("strrchr",  sig!(ret: u8p(), params: [p!("s", u8p()), p!("c", i32_())]));
    m.insert("strstr",   sig!(ret: u8p(), params: [p!("haystack", u8p()), p!("needle", u8p())]));
    m.insert("strtok",   sig!(ret: u8p(), params: [p!("s", u8p()), p!("delim", u8p())]));
    m.insert("memcpy",   sig!(ret: vp(), params: [p!("dst", vp()), p!("src", vp()), p!("n", sz())]));
    m.insert("memmove",  sig!(ret: vp(), params: [p!("dst", vp()), p!("src", vp()), p!("n", sz())]));
    m.insert("memset",   sig!(ret: vp(), params: [p!("s", vp()), p!("c", i32_()), p!("n", sz())]));
    m.insert("memcmp",   sig!(ret: i32_(), params: [p!("a", vp()), p!("b", vp()), p!("n", sz())]));

    // ── POSIX / Unix ──────────────────────────────────────────────────────────
    m.insert("write",    sig!(ret: i64_(), params: [p!("fd", i32_()), p!("buf", vp()), p!("n", sz())]));
    m.insert("read",     sig!(ret: i64_(), params: [p!("fd", i32_()), p!("buf", vp()), p!("n", sz())]));
    m.insert("open",     sig!(ret: i32_(), params: [p!("path", u8p()), p!("flags", i32_())], variadic: true));
    m.insert("close",    sig!(ret: i32_(), params: [p!("fd", i32_())]));
    m.insert("getenv",   sig!(ret: u8p(), params: [p!("name", u8p())]));
    m.insert("system",   sig!(ret: i32_(), params: [p!("cmd", u8p())]));

    // ── C++ runtime (demangled names already handled upstream) ────────────────
    m.insert("__stack_chk_fail",        sig!(ret: Void, params: []));
    m.insert("__cxa_allocate_exception",sig!(ret: vp(), params: [p!("size", sz())]));
    m.insert("__cxa_throw",             sig!(ret: Void, params: [p!("obj", vp()), p!("tinfo", vp()), p!("dest", vp())]));

    m
}
