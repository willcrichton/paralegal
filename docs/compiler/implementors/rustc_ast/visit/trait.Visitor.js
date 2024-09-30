(function() {var implementors = {
"clippy_utils":[["impl <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'_&gt; for <a class=\"struct\" href=\"clippy_utils/ast_utils/ident_iter/struct.IdentCollector.html\" title=\"struct clippy_utils::ast_utils::ident_iter::IdentCollector\">IdentCollector</a>"]],
"rustc_ast_lowering":[["impl&lt;'ast&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'ast&gt; for <a class=\"struct\" href=\"rustc_ast_lowering/lifetime_collector/struct.LifetimeCollectVisitor.html\" title=\"struct rustc_ast_lowering::lifetime_collector::LifetimeCollectVisitor\">LifetimeCollectVisitor</a>&lt;'ast&gt;"]],
"rustc_ast_passes":[["impl&lt;'a&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'a&gt; for <a class=\"struct\" href=\"rustc_ast_passes/feature_gate/struct.PostExpansionVisitor.html\" title=\"struct rustc_ast_passes::feature_gate::PostExpansionVisitor\">PostExpansionVisitor</a>&lt;'a&gt;"],["impl&lt;'a&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'a&gt; for <a class=\"struct\" href=\"rustc_ast_passes/ast_validation/struct.AstValidator.html\" title=\"struct rustc_ast_passes::ast_validation::AstValidator\">AstValidator</a>&lt;'a&gt;"],["impl&lt;'ast&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'ast&gt; for <a class=\"struct\" href=\"rustc_ast_passes/node_count/struct.NodeCounter.html\" title=\"struct rustc_ast_passes::node_count::NodeCounter\">NodeCounter</a>"],["impl&lt;'a&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'a&gt; for <a class=\"struct\" href=\"rustc_ast_passes/show_span/struct.ShowSpanVisitor.html\" title=\"struct rustc_ast_passes::show_span::ShowSpanVisitor\">ShowSpanVisitor</a>&lt;'a&gt;"]],
"rustc_builtin_macros":[["impl&lt;'a&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'a&gt; for <a class=\"struct\" href=\"rustc_builtin_macros/test_harness/struct.InnerItemLinter.html\" title=\"struct rustc_builtin_macros::test_harness::InnerItemLinter\">InnerItemLinter</a>&lt;'_&gt;"],["impl&lt;'ast&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'ast&gt; for <a class=\"struct\" href=\"rustc_builtin_macros/cfg_eval/struct.CfgFinder.html\" title=\"struct rustc_builtin_macros::cfg_eval::CfgFinder\">CfgFinder</a>"],["impl&lt;'a&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'a&gt; for <a class=\"struct\" href=\"rustc_builtin_macros/proc_macro_harness/struct.CollectProcMacros.html\" title=\"struct rustc_builtin_macros::proc_macro_harness::CollectProcMacros\">CollectProcMacros</a>&lt;'a&gt;"],["impl&lt;'a, 'b&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'a&gt; for <a class=\"struct\" href=\"rustc_builtin_macros/deriving/default/struct.DetectNonVariantDefaultAttr.html\" title=\"struct rustc_builtin_macros::deriving::default::DetectNonVariantDefaultAttr\">DetectNonVariantDefaultAttr</a>&lt;'a, 'b&gt;"]],
"rustc_lint":[["impl&lt;'a, T: <a class=\"trait\" href=\"rustc_lint/passes/trait.EarlyLintPass.html\" title=\"trait rustc_lint::passes::EarlyLintPass\">EarlyLintPass</a>&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'a&gt; for <a class=\"struct\" href=\"rustc_lint/early/struct.EarlyContextAndPass.html\" title=\"struct rustc_lint::early::EarlyContextAndPass\">EarlyContextAndPass</a>&lt;'a, T&gt;"]],
"rustc_passes":[["impl&lt;'v&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'v&gt; for <a class=\"struct\" href=\"rustc_passes/hir_stats/struct.StatCollector.html\" title=\"struct rustc_passes::hir_stats::StatCollector\">StatCollector</a>&lt;'v&gt;"],["impl&lt;'ast&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'ast&gt; for <a class=\"struct\" href=\"rustc_passes/debugger_visualizer/struct.DebuggerVisualizerCollector.html\" title=\"struct rustc_passes::debugger_visualizer::DebuggerVisualizerCollector\">DebuggerVisualizerCollector</a>&lt;'_&gt;"]],
"rustc_resolve":[["impl&lt;'r, 'ast, 'tcx&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'ast&gt; for <a class=\"struct\" href=\"rustc_resolve/effective_visibilities/struct.EffectiveVisibilitiesVisitor.html\" title=\"struct rustc_resolve::effective_visibilities::EffectiveVisibilitiesVisitor\">EffectiveVisibilitiesVisitor</a>&lt;'ast, 'r, 'tcx&gt;"],["impl&lt;'a: 'ast, 'ast, 'tcx&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'ast&gt; for <a class=\"struct\" href=\"rustc_resolve/late/struct.LateResolutionVisitor.html\" title=\"struct rustc_resolve::late::LateResolutionVisitor\">LateResolutionVisitor</a>&lt;'a, '_, 'ast, 'tcx&gt;"],["impl&lt;'a, 'b, 'tcx&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'a&gt; for <a class=\"struct\" href=\"rustc_resolve/def_collector/struct.DefCollector.html\" title=\"struct rustc_resolve::def_collector::DefCollector\">DefCollector</a>&lt;'a, 'b, 'tcx&gt;"],["impl&lt;'a, 'b, 'tcx&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'a&gt; for <a class=\"struct\" href=\"rustc_resolve/check_unused/struct.UnusedImportCheckVisitor.html\" title=\"struct rustc_resolve::check_unused::UnusedImportCheckVisitor\">UnusedImportCheckVisitor</a>&lt;'a, 'b, 'tcx&gt;"],["impl&lt;'ast&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'ast&gt; for <a class=\"struct\" href=\"rustc_resolve/late/struct.LifetimeCountVisitor.html\" title=\"struct rustc_resolve::late::LifetimeCountVisitor\">LifetimeCountVisitor</a>&lt;'_, '_, '_&gt;"],["impl&lt;'tcx&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'tcx&gt; for <a class=\"struct\" href=\"rustc_resolve/diagnostics/struct.UsePlacementFinder.html\" title=\"struct rustc_resolve::diagnostics::UsePlacementFinder\">UsePlacementFinder</a>"],["impl&lt;'a, 'b, 'tcx&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'b&gt; for <a class=\"struct\" href=\"rustc_resolve/build_reduced_graph/struct.BuildReducedGraphVisitor.html\" title=\"struct rustc_resolve::build_reduced_graph::BuildReducedGraphVisitor\">BuildReducedGraphVisitor</a>&lt;'a, 'b, 'tcx&gt;"]],
"rustfmt_nightly":[["impl&lt;'a, 'ast: 'a&gt; <a class=\"trait\" href=\"rustc_ast/visit/trait.Visitor.html\" title=\"trait rustc_ast::visit::Visitor\">Visitor</a>&lt;'ast&gt; for <a class=\"struct\" href=\"rustfmt_nightly/modules/visitor/struct.CfgIfVisitor.html\" title=\"struct rustfmt_nightly::modules::visitor::CfgIfVisitor\">CfgIfVisitor</a>&lt;'a&gt;"]]
};if (window.register_implementors) {window.register_implementors(implementors);} else {window.pending_implementors = implementors;}})()