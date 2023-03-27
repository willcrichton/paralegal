//! Command line arguments and parsing.

/// Top level command line arguments
#[derive(serde::Serialize, serde::Deserialize, clap::Args)]
pub struct Args {
    /// Print additional logging output (up to the "info" level)
    #[clap(short, long, env = "DFPP_VERBOSE")]
    verbose: bool,
    /// Print additional logging output (up to the "debug" level).
    ///
    /// Passing this flag (or env variable) with no value will enable debug
    /// output globally. You may instead pass the name of a specific target
    /// function and then only during analysis of that function the debug output
    /// is enabled.
    #[clap(long, env = "DFPP_DEBUG")]
    debug: Option<Option<String>>,
    /// Where to write the resulting forge code to (defaults to `analysis_result.frg`)
    #[clap(long, default_value = "analysis_result.frg")]
    result_path: std::path::PathBuf,
    /// Additional arguments that control the flow analysis specifically
    #[clap(flatten, next_help_heading = "Flow Analysis")]
    anactrl: AnalysisCtrl,
    /// Additional arguments that control the generation and composition of the model
    #[clap(flatten, next_help_heading = "Model Generation")]
    modelctrl: ModelCtrl,
    /// Additional arguments that control debug args specifically
    #[clap(flatten, next_help_heading = "Debugging and Testing")]
    dbg: DbgArgs,
}

/// How a specific logging level was configured. (currently only used for the
/// `--debug` level)
pub enum LogLevelConfig<'a> {
    /// Logging for this level is only enabled for a specific target function
    Targeted(&'a str),
    /// Logging for this level is not directly enabled
    Disabled,
    /// Logging for this level was directly enabled
    Enabled,
}

impl LogLevelConfig<'_> {
    pub fn is_enabled(&self) -> bool {
        matches!(self, LogLevelConfig::Targeted(..) | LogLevelConfig::Enabled)
    }
}

impl Args {
    /// Returns the configuration specified for the `--debug` option
    pub fn debug(&self) -> LogLevelConfig {
        match self.debug.as_ref() {
            None => LogLevelConfig::Disabled,
            Some(i) => i
                .as_ref()
                .and_then(|s| (s != "").then_some(s.as_str()))
                .map_or(LogLevelConfig::Enabled, LogLevelConfig::Targeted),
        }
    }
    pub fn dbg(&self) -> &DbgArgs {
        &self.dbg
    }
    pub fn anactrl(&self) -> &AnalysisCtrl {
        &self.anactrl
    }
    pub fn modelctrl(&self) -> &ModelCtrl {
        &self.modelctrl
    }
    pub fn result_path(&self) -> &std::path::Path {
        self.result_path.as_path()
    }
    pub fn verbose(&self) -> bool {
        self.verbose
    }
}

#[derive(serde::Serialize, serde::Deserialize, clap::Args)]
pub struct ModelCtrl {
    /// A JSON file from which to load additional annotations. Whereas normally
    /// annotation can only be placed on crate-local items, these can also be
    /// placed on third party items, such as functions from the stdlib.
    ///
    /// The file is expected to contain a `HashMap<Identifier, (Vec<Annotation>,
    /// ObjectType)>`, which is the same type as `annotations` field from the
    /// `ProgramDescription` struct. It uses the `serde` derived serializer. An
    /// example for the format can be generated by running dfpp with
    /// `dump_serialized_flow_graph`.
    #[clap(long, env)]
    external_annotations: Option<std::path::PathBuf>,
}

impl ModelCtrl {
    pub fn external_annotations(&self) -> Option<&std::path::Path> {
        self.external_annotations.as_ref().map(|p| p.as_path())
    }
}

/// Arguments that control the flow analysis
#[derive(serde::Serialize, serde::Deserialize, clap::Args)]
pub struct AnalysisCtrl {
    /// Disables all recursive analysis (both dfpps inlining as well as
    /// Flowistry's recursive analysis)
    #[clap(long, env)]
    no_recursive_analysis: bool,
    /// Make flowistry use a recursive analysis strategy. We turn this off by
    /// default, because we perform the recursion by ourselves and doing it
    /// twice has lead to bugs.
    #[clap(long, env)]
    recursive_flowistry: bool,
}

/// Arguments that control the output of debug information or output to be
/// consumed for testing.
#[derive(serde::Serialize, serde::Deserialize, clap::Args)]
pub struct DbgArgs {
    /// Dumps a table representing retrieved Flowistry matrices to stdout.
    #[clap(long, env)]
    dump_flowistry_matrix: bool,
    /// Dumps a dot graph representation of the finely granular, inlined flow of each controller.
    /// Unlike `dump_call_only_flow` this contains also statements and non-call
    /// terminators. It is also created differently (using dependency
    /// resolution) and has not been tested in a while and shouldn't be relied upon.
    #[clap(long, env)]
    dump_inlined_function_flow: bool,
    /// Dumps a dot graph representation of the dataflow between function calls
    /// calculated for each controller to <name of controller>.call-only-flow.gv
    #[clap(long, env)]
    dump_call_only_flow: bool,
    /// Deprecated alias for `dump_call_only_flow`
    #[clap(long, env)]
    dump_non_transitive_graph: bool,
    /// For each controller dumps the calculated dataflow graphs as well as
    /// information about the MIR to <name of controller>.ntgb.json. Can be
    /// deserialized with `crate::dbg::read_non_transitive_graph_and_body`.
    #[clap(long, env)]
    dump_serialized_non_transitive_graph: bool,
    /// Dump a complete `crate::desc::ProgramDescription` in serialized (json)
    /// format to "flow-graph.json". Used for testing.
    #[clap(long, env)]
    dump_serialized_flow_graph: bool,
    /// For each controller dump a dot representation for each [`mir::Body`] as
    /// provided by rustc
    #[clap(long, env)]
    dump_ctrl_mir: bool,
}

impl DbgArgs {
    pub fn dump_ctrl_mir(&self) -> bool {
        self.dump_ctrl_mir
    }
    pub fn dump_serialized_non_transitive_graph(&self) -> bool {
        self.dump_serialized_non_transitive_graph
    }
    pub fn dump_call_only_flow(&self) -> bool {
        self.dump_call_only_flow || self.dump_non_transitive_graph
    }
    pub fn dump_serialized_flow_graph(&self) -> bool {
        self.dump_serialized_flow_graph
    }
}
