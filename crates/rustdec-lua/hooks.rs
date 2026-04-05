//! Pipeline extension points.
//!
//! Each variant maps to a specific moment in the pipeline where a Lua
//! script can observe or modify data.
//! V1 (stub): this enum is declared but never fired.

/// Hook point in the analysis pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Hook {
    /// A binary has just been loaded and parsed.
    OnBinaryLoaded,

    /// A function has been detected (symbol or call-site).
    OnFunctionDetected,

    /// The CFG of a function has just been built.
    OnCfgBuilt,

    /// A function has just been lifted to IR.
    OnFunctionLifted,

    /// The pseudo-code of a function has just been emitted.
    OnCodeEmitted,

    /// The full analysis of the binary is complete.
    OnAnalysisComplete,
}

impl std::fmt::Display for Hook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::OnBinaryLoaded     => "on_binary_loaded",
            Self::OnFunctionDetected => "on_function_detected",
            Self::OnCfgBuilt         => "on_cfg_built",
            Self::OnFunctionLifted   => "on_function_lifted",
            Self::OnCodeEmitted      => "on_code_emitted",
            Self::OnAnalysisComplete => "on_analysis_complete",
        };
        f.write_str(s)
    }
}
