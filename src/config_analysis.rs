use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use ls_types::{Range, Uri};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpression, ArrayExpressionElement, ArrowFunctionBody, AssignmentExpression,
    AssignmentTarget, BindingPattern, BlockStatement, CatchClause, Expression, ForInStatement,
    ForOfStatement, ForStatement, ForStatementInit, ForStatementLeft, ImportDeclaration,
    ImportDeclarationSpecifier, MemberExpression, ObjectExpression, ObjectPropertyKind, Program,
    PropertyKey, SimpleAssignmentTarget, Statement, SwitchStatement, UpdateExpression,
    VariableDeclaration, VariableDeclarator,
};
use oxc_ast_visit::{
    walk::{walk_assignment_expression, walk_block_statement, walk_update_expression},
    Visit,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};

use crate::manager::CssVariableManager;
use crate::parsers::css::{parse_css_snippet, CssParseContext};
use crate::types::{offset_to_position, CssVariable};

pub(crate) const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const ASTRO_CONFIG_NAMES: &[&str] = &[
    "astro.config.js",
    "astro.config.mjs",
    "astro.config.cjs",
    "astro.config.ts",
    "astro.config.mts",
    "astro.config.cts",
];
const VITE_CONFIG_NAMES: &[&str] = &[
    "vite.config.js",
    "vite.config.mjs",
    "vite.config.cjs",
    "vite.config.ts",
    "vite.config.mts",
    "vite.config.cts",
];
const VITE_PREPROCESSOR: &str = "scss";
const MAX_STATIC_STRING_DEPTH: usize = 16;
const MAX_STATIC_STRING_VISITS: usize = 64;
const MAX_STATIC_STRUCTURE_VISITS: usize = 1024;
static CONFIG_PARSE_COUNT: AtomicU64 = AtomicU64::new(0);
static OVERSIZED_CONFIG_SKIP_COUNT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
enum ConfigKind {
    Astro,
    Vite,
}

#[derive(Clone, Copy)]
pub(crate) enum ConfigVariableSource {
    AstroFont,
    ViteScssAdditionalData,
}

impl ConfigVariableSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::AstroFont => "Astro font configuration",
            Self::ViteScssAdditionalData => "Vite SCSS additionalData",
        }
    }
}

fn config_kind(path: &Path) -> Option<ConfigKind> {
    let name = path.file_name()?.to_str()?;
    if ASTRO_CONFIG_NAMES.contains(&name) {
        Some(ConfigKind::Astro)
    } else if VITE_CONFIG_NAMES.contains(&name) {
        Some(ConfigKind::Vite)
    } else {
        None
    }
}

fn supports_commonjs(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("js" | "cjs" | "ts" | "cts")
    )
}

/// Return whether a path is a supported framework configuration source.
pub fn is_supported_config_path(path: &Path) -> bool {
    config_kind(path).is_some()
}

pub(crate) fn config_variable_source(uri: &Uri) -> Option<ConfigVariableSource> {
    match config_kind(Path::new(uri.path().as_str()))? {
        ConfigKind::Astro => Some(ConfigVariableSource::AstroFont),
        ConfigKind::Vite => Some(ConfigVariableSource::ViteScssAdditionalData),
    }
}

/// Parse a recognized framework configuration file without executing it.
pub async fn parse_config_document(
    text: &str,
    uri: &Uri,
    manager: &CssVariableManager,
) -> Result<(), String> {
    let path = Path::new(uri.path().as_str());
    if config_kind(path).is_none() {
        return Ok(());
    }

    if text.len() > MAX_CONFIG_BYTES {
        let skip_count = OVERSIZED_CONFIG_SKIP_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::debug!(
            uri = ?uri,
            bytes = text.len(),
            limit = MAX_CONFIG_BYTES,
            skip_count,
            "configuration analysis retained the last valid state for an oversized source"
        );
        return Ok(());
    }

    let Some(extracted) = extract_config_variables(text, path, uri)? else {
        return Ok(());
    };
    let mut variables: Vec<_> = extracted
        .variables
        .into_iter()
        .map(|extracted| CssVariable {
            name: extracted.name,
            value: String::new(),
            uri: uri.clone(),
            range: span_to_range(text, extracted.declaration_span),
            name_range: Some(span_to_range(text, extracted.name_span)),
            value_range: None,
            selector: ":root".to_string(),
            important: false,
            inline: false,
            source_position: extracted.declaration_span.start as usize,
        })
        .collect();

    let mut usages = Vec::new();
    if !extracted.css_snippets.is_empty() {
        let snippet_manager = CssVariableManager::new(manager.get_config().await);
        for snippet in extracted.css_snippets {
            parse_css_snippet(CssParseContext {
                css_text: &snippet.text,
                full_text: text,
                uri,
                manager: &snippet_manager,
                base_offset: snippet.content_span.start as usize,
                inline: false,
                usage_context_override: None,
                dom_node: None,
            })
            .await?;
        }
        variables.extend(snippet_manager.get_document_variables(uri).await);
        usages.extend(snippet_manager.get_document_usages(uri).await);
    }

    manager
        .replace_document_analysis(uri, variables, usages)
        .await
}

fn extract_config_variables(
    text: &str,
    path: &Path,
    uri: &Uri,
) -> Result<Option<ConfigExtraction>, String> {
    let started = Instant::now();
    let parse_count = CONFIG_PARSE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let kind = config_kind(path)
        .ok_or_else(|| format!("Unsupported configuration source: {}", path.display()))?;
    let source_type = SourceType::from_path(path)
        .map_err(|_| format!("Unsupported configuration source type: {}", path.display()))?;
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, source_type).parse();

    let diagnostic_count = parsed.diagnostics.len();
    if diagnostic_count > 0 && (parsed.panicked || parsed.program.body.is_empty()) {
        tracing::debug!(
            uri = ?uri,
            errors = diagnostic_count,
            parse_count,
            elapsed_micros = started.elapsed().as_micros(),
            "configuration analysis retained the last valid state after a catastrophic parse"
        );
        return Ok(None);
    }
    if diagnostic_count > 0 {
        tracing::debug!(
            uri = ?uri,
            errors = diagnostic_count,
            parse_count,
            "configuration analysis is using Oxc's recoverable AST"
        );
    }

    let module_source = match kind {
        ConfigKind::Astro => "astro/config",
        ConfigKind::Vite => "vite",
    };
    let mut imports = DefineConfigImportCollector::new(module_source);
    imports.visit_program(&parsed.program);
    if supports_commonjs(path) {
        imports.collect_commonjs_program(&parsed.program);
    }
    imports.reject_mutated_helpers(&parsed.program);
    let static_resolver = collect_static_resolver(&parsed.program, text);
    let expression_resolver = StaticExpressionResolver::new(&static_resolver);

    let has_esm_default = parsed
        .program
        .body
        .iter()
        .any(|statement| matches!(statement, Statement::ExportDefaultDeclaration(_)));
    let DefineConfigImportCollector {
        define_config_bindings,
        define_config_namespaces,
        ..
    } = imports;
    let extracted = match kind {
        ConfigKind::Astro => {
            let mut extractor = AstroFontExtractor {
                source: text,
                define_config_bindings,
                define_config_namespaces,
                expression_resolver,
                variables: Vec::new(),
            };
            extractor.extract_esm_program(&parsed.program);
            if supports_commonjs(path) && !has_esm_default {
                extractor.extract_commonjs_program(&parsed.program);
            }
            ConfigExtraction {
                variables: extractor.variables,
                css_snippets: Vec::new(),
            }
        }
        ConfigKind::Vite => {
            let mut extractor = ViteAdditionalDataExtractor {
                source: text,
                define_config_bindings,
                define_config_namespaces,
                expression_resolver,
                css_snippets: Vec::new(),
            };
            extractor.extract_esm_program(&parsed.program);
            if supports_commonjs(path) && !has_esm_default {
                extractor.extract_commonjs_program(&parsed.program);
            }
            ConfigExtraction {
                variables: Vec::new(),
                css_snippets: extractor.css_snippets,
            }
        }
    };
    tracing::debug!(
        uri = ?uri,
        parse_count,
        diagnostic_count,
        variables = extracted.variables.len(),
        css_snippets = extracted.css_snippets.len(),
        elapsed_micros = started.elapsed().as_micros(),
        "configuration analysis completed"
    );
    Ok(Some(extracted))
}

#[derive(Default)]
struct ConfigExtraction {
    variables: Vec<ExtractedVariable>,
    css_snippets: Vec<ExtractedCssSnippet>,
}

struct ExtractedVariable {
    name: String,
    declaration_span: Span,
    name_span: Span,
}

struct ExtractedCssSnippet {
    text: String,
    content_span: Span,
}

#[derive(Clone)]
struct ResolvedStaticString {
    content_span: Span,
    available_after: u32,
}

#[derive(Clone, Copy)]
struct StaticExpressionResolver<'a> {
    strings: &'a HashMap<String, ResolvedStaticString>,
    bindings: &'a HashMap<String, &'a Expression<'a>>,
    assigned_names: &'a HashSet<String>,
}

enum ResolvedProperty<'a> {
    Known {
        value: &'a Expression<'a>,
        declaration_span: Span,
    },
    Unknown,
}

impl<'a> StaticExpressionResolver<'a> {
    fn new(static_strings: &'a StaticStringResolver<'a, '_>) -> Self {
        Self {
            strings: &static_strings.resolved_strings,
            bindings: &static_strings.bindings,
            assigned_names: &static_strings.assigned_names,
        }
    }

    fn resolve_expression(
        &self,
        expression: &'a Expression<'a>,
        depth: usize,
        visits: &mut usize,
        resolving: &mut HashSet<String>,
    ) -> Option<&'a Expression<'a>> {
        if depth >= MAX_STATIC_STRING_DEPTH || *visits >= MAX_STATIC_STRING_VISITS {
            return None;
        }
        *visits += 1;
        let expression = unwrap_expression(expression);
        let Expression::Identifier(identifier) = expression else {
            return Some(expression);
        };
        let name = identifier.name.as_str();
        if self.assigned_names.contains(name) || !resolving.insert(name.to_string()) {
            return None;
        }
        let init = self.bindings.get(name)?;
        if init.span().end > identifier.span.end {
            resolving.remove(name);
            return None;
        }
        let result = self.resolve_expression(init, depth + 1, visits, resolving);
        resolving.remove(name);
        result
    }

    fn as_object(&self, expression: &'a Expression<'a>) -> Option<&'a ObjectExpression<'a>> {
        let mut visits = 0;
        let mut resolving = HashSet::new();
        self.resolve_expression(expression, 0, &mut visits, &mut resolving)
            .and_then(as_object_expression)
    }

    fn as_array(&self, expression: &'a Expression<'a>) -> Option<&'a ArrayExpression<'a>> {
        let mut visits = 0;
        let mut resolving = HashSet::new();
        match self.resolve_expression(expression, 0, &mut visits, &mut resolving)? {
            Expression::ArrayExpression(array) => Some(array.as_ref()),
            _ => None,
        }
    }

    fn property(
        &self,
        object: &'a ObjectExpression<'a>,
        name: &str,
    ) -> Option<ResolvedProperty<'a>> {
        let mut visits = 0;
        self.property_inner(object, name, 0, &mut visits)
    }

    fn property_inner(
        &self,
        object: &'a ObjectExpression<'a>,
        name: &str,
        depth: usize,
        visits: &mut usize,
    ) -> Option<ResolvedProperty<'a>> {
        if depth >= MAX_STATIC_STRING_DEPTH || *visits >= MAX_STATIC_STRUCTURE_VISITS {
            return Some(ResolvedProperty::Unknown);
        }
        *visits += 1;

        let mut unknown_after_match = false;
        for property in object.properties.iter().rev() {
            match property {
                ObjectPropertyKind::ObjectProperty(property) => {
                    let matches = if property.computed {
                        property.key.is_specific_string_literal(name)
                    } else {
                        property_key_matches(&property.key, name)
                    };
                    if matches {
                        return (!unknown_after_match).then_some(ResolvedProperty::Known {
                            value: &property.value,
                            declaration_span: property.span,
                        });
                    }
                    if property.computed && !matches!(property.key, PropertyKey::StringLiteral(_)) {
                        unknown_after_match = true;
                    }
                }
                ObjectPropertyKind::SpreadProperty(spread) => {
                    let Some(spread_object) = self.as_object(&spread.argument) else {
                        unknown_after_match = true;
                        continue;
                    };
                    match self.property_inner(spread_object, name, depth + 1, visits) {
                        Some(known @ ResolvedProperty::Known { .. }) if !unknown_after_match => {
                            return Some(known);
                        }
                        Some(_) => return Some(ResolvedProperty::Unknown),
                        None => {}
                    }
                }
            }
        }
        if unknown_after_match {
            Some(ResolvedProperty::Unknown)
        } else {
            None
        }
    }

    fn property_value(
        &self,
        object: &'a ObjectExpression<'a>,
        name: &str,
    ) -> Option<&'a Expression<'a>> {
        match self.property(object, name)? {
            ResolvedProperty::Known { value, .. } => Some(value),
            ResolvedProperty::Unknown => None,
        }
    }
}

#[derive(Default)]
struct AssignmentTargetNameCollector {
    names: HashSet<String>,
}

impl<'a> Visit<'a> for AssignmentTargetNameCollector {
    fn visit_identifier_reference(&mut self, identifier: &oxc_ast::ast::IdentifierReference<'a>) {
        self.names.insert(identifier.name.as_str().to_string());
    }

    fn visit_member_expression(&mut self, _expression: &MemberExpression<'a>) {}

    fn visit_assignment_target_with_default(
        &mut self,
        target: &oxc_ast::ast::AssignmentTargetWithDefault<'a>,
    ) {
        self.visit_assignment_target(&target.binding);
    }

    fn visit_assignment_target_property_identifier(
        &mut self,
        property: &oxc_ast::ast::AssignmentTargetPropertyIdentifier<'a>,
    ) {
        self.visit_identifier_reference(&property.binding);
    }

    fn visit_assignment_target_property_property(
        &mut self,
        property: &oxc_ast::ast::AssignmentTargetPropertyProperty<'a>,
    ) {
        self.visit_assignment_target_maybe_default(&property.binding);
    }
}

#[derive(Default)]
struct BindingNameCollector {
    names: HashSet<String>,
}

impl<'a> Visit<'a> for BindingNameCollector {
    fn visit_binding_identifier(&mut self, identifier: &oxc_ast::ast::BindingIdentifier<'a>) {
        self.names.insert(identifier.name.as_str().to_string());
    }

    fn visit_expression(&mut self, _expression: &Expression<'a>) {}
}

struct AssignedBindingCollector {
    tracked_names: HashSet<String>,
    names: HashSet<String>,
    shadowed_scopes: Vec<HashSet<String>>,
    tracked_member_property: Option<&'static str>,
}

impl AssignedBindingCollector {
    fn new(tracked_names: HashSet<String>) -> Self {
        Self {
            tracked_names,
            names: HashSet::new(),
            shadowed_scopes: Vec::new(),
            tracked_member_property: None,
        }
    }

    fn with_member_property(mut self, property: &'static str) -> Self {
        self.tracked_member_property = Some(property);
        self
    }

    fn record_names(&mut self, names: HashSet<String>) {
        for name in names {
            let shadowed = self
                .shadowed_scopes
                .iter()
                .rev()
                .any(|scope| scope.contains(&name));
            if self.tracked_names.contains(&name) && !shadowed {
                self.names.insert(name);
            }
        }
    }

    fn record_assignment_target<'a>(&mut self, target: &AssignmentTarget<'a>) {
        let mut collector = AssignmentTargetNameCollector::default();
        collector.visit_assignment_target(target);
        self.record_names(collector.names);

        let Some(member) = target.as_member_expression() else {
            return;
        };
        self.record_member_assignment(member);
    }

    fn record_simple_assignment_target<'a>(&mut self, target: &SimpleAssignmentTarget<'a>) {
        let mut collector = AssignmentTargetNameCollector::default();
        collector.visit_simple_assignment_target(target);
        self.record_names(collector.names);
        if let Some(member) = target.as_member_expression() {
            self.record_member_assignment(member);
        }
    }

    fn record_member_assignment(&mut self, member: &MemberExpression<'_>) {
        if self
            .tracked_member_property
            .is_some_and(|property| member.static_property_name() != Some(property))
        {
            return;
        }

        let mut object = member.object();
        loop {
            match unwrap_expression(object) {
                Expression::Identifier(identifier) => {
                    self.record_names(HashSet::from([identifier.name.as_str().to_string()]));
                    return;
                }
                expression => {
                    let Some(parent) = expression.as_member_expression() else {
                        return;
                    };
                    object = parent.object();
                }
            }
        }
    }

    fn record_for_statement_left<'a>(&mut self, left: &ForStatementLeft<'a>) {
        let mut collector = AssignmentTargetNameCollector::default();
        collector.visit_for_statement_left(left);
        self.record_names(collector.names);
    }

    fn declaration_names(declaration: &VariableDeclaration<'_>) -> HashSet<String> {
        let mut collector = BindingNameCollector::default();
        for declarator in &declaration.declarations {
            collector.visit_binding_pattern(&declarator.id);
        }
        collector.names
    }

    fn lexical_declaration_names(declaration: &VariableDeclaration<'_>) -> HashSet<String> {
        if declaration.kind.is_var() {
            HashSet::new()
        } else {
            Self::declaration_names(declaration)
        }
    }

    fn block_shadow_names(block: &BlockStatement<'_>) -> HashSet<String> {
        let mut names = HashSet::new();
        for statement in &block.body {
            match statement {
                Statement::VariableDeclaration(declaration) => {
                    names.extend(Self::lexical_declaration_names(declaration));
                }
                Statement::FunctionDeclaration(function) => {
                    if let Some(identifier) = &function.id {
                        names.insert(identifier.name.as_str().to_string());
                    }
                }
                Statement::ClassDeclaration(class) => {
                    if let Some(identifier) = &class.id {
                        names.insert(identifier.name.as_str().to_string());
                    }
                }
                _ => {}
            }
        }
        names
    }

    fn switch_shadow_names(statement: &SwitchStatement<'_>) -> HashSet<String> {
        let mut names = HashSet::new();
        for case in &statement.cases {
            for statement in &case.consequent {
                match statement {
                    Statement::VariableDeclaration(declaration) => {
                        names.extend(Self::lexical_declaration_names(declaration));
                    }
                    Statement::FunctionDeclaration(function) => {
                        if let Some(identifier) = &function.id {
                            names.insert(identifier.name.as_str().to_string());
                        }
                    }
                    Statement::ClassDeclaration(class) => {
                        if let Some(identifier) = &class.id {
                            names.insert(identifier.name.as_str().to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        names
    }

    fn with_shadowed_scope(&mut self, names: HashSet<String>, visit: impl FnOnce(&mut Self)) {
        self.shadowed_scopes.push(names);
        visit(self);
        self.shadowed_scopes.pop();
    }
}

impl<'a> Visit<'a> for AssignedBindingCollector {
    fn visit_assignment_expression(&mut self, expression: &AssignmentExpression<'a>) {
        self.record_assignment_target(&expression.left);
        walk_assignment_expression(self, expression);
    }

    fn visit_update_expression(&mut self, expression: &UpdateExpression<'a>) {
        self.record_simple_assignment_target(&expression.argument);
        walk_update_expression(self, expression);
    }

    fn visit_function_body(&mut self, body: &oxc_ast::ast::FunctionBody<'a>) {
        oxc_ast_visit::walk::walk_function_body(self, body);
    }

    fn visit_block_statement(&mut self, block: &BlockStatement<'a>) {
        let names = Self::block_shadow_names(block);
        self.with_shadowed_scope(names, |collector| walk_block_statement(collector, block));
    }

    fn visit_catch_clause(&mut self, clause: &CatchClause<'a>) {
        let mut names = BindingNameCollector::default();
        if let Some(parameter) = &clause.param {
            names.visit_binding_pattern(&parameter.pattern);
        }
        self.with_shadowed_scope(names.names, |collector| {
            collector.visit_block_statement(&clause.body);
        });
    }

    fn visit_switch_statement(&mut self, statement: &SwitchStatement<'a>) {
        self.visit_expression(&statement.discriminant);
        let names = Self::switch_shadow_names(statement);
        self.with_shadowed_scope(names, |collector| {
            collector.visit_switch_cases(&statement.cases);
        });
    }

    fn visit_for_statement(&mut self, statement: &ForStatement<'a>) {
        let shadowed = statement
            .init
            .as_ref()
            .and_then(|init| match init {
                ForStatementInit::VariableDeclaration(declaration) => {
                    Some(Self::lexical_declaration_names(declaration))
                }
                _ => None,
            })
            .unwrap_or_default();
        self.with_shadowed_scope(shadowed, |collector| {
            if let Some(init) = &statement.init {
                collector.visit_for_statement_init(init);
            }
            if let Some(test) = &statement.test {
                collector.visit_expression(test);
            }
            if let Some(update) = &statement.update {
                collector.visit_expression(update);
            }
            collector.visit_statement(&statement.body);
        });
    }

    fn visit_for_in_statement(&mut self, statement: &ForInStatement<'a>) {
        let shadowed = match &statement.left {
            ForStatementLeft::VariableDeclaration(declaration) => {
                Self::lexical_declaration_names(declaration)
            }
            _ => HashSet::new(),
        };
        self.with_shadowed_scope(shadowed, |collector| {
            if matches!(&statement.left, ForStatementLeft::VariableDeclaration(_)) {
                collector.visit_for_statement_left(&statement.left);
            } else {
                collector.record_for_statement_left(&statement.left);
            }
            collector.visit_expression(&statement.right);
            collector.visit_statement(&statement.body);
        });
    }

    fn visit_for_of_statement(&mut self, statement: &ForOfStatement<'a>) {
        let shadowed = match &statement.left {
            ForStatementLeft::VariableDeclaration(declaration) => {
                Self::lexical_declaration_names(declaration)
            }
            _ => HashSet::new(),
        };
        self.with_shadowed_scope(shadowed, |collector| {
            if matches!(&statement.left, ForStatementLeft::VariableDeclaration(_)) {
                collector.visit_for_statement_left(&statement.left);
            } else {
                collector.record_for_statement_left(&statement.left);
            }
            collector.visit_expression(&statement.right);
            collector.visit_statement(&statement.body);
        });
    }
}

struct StaticStringResolver<'a, 's> {
    source: &'s str,
    bindings: HashMap<String, &'a Expression<'a>>,
    resolved_strings: HashMap<String, ResolvedStaticString>,
    assigned_names: HashSet<String>,
}

impl<'a, 's> StaticStringResolver<'a, 's> {
    fn from_program(program: &'a Program<'a>, source: &'s str) -> Self {
        let mut bindings = HashMap::new();
        let mut duplicate_names = HashSet::new();

        for statement in &program.body {
            let Statement::VariableDeclaration(declaration) = statement else {
                continue;
            };
            if !declaration.kind.is_const() || declaration.declare {
                continue;
            }

            for declarator in &declaration.declarations {
                let BindingPattern::BindingIdentifier(identifier) = &declarator.id else {
                    continue;
                };
                let Some(init) = declarator.init.as_ref() else {
                    continue;
                };
                let name = identifier.name.as_str().to_string();
                if bindings.insert(name.clone(), init).is_some() {
                    duplicate_names.insert(name);
                }
            }
        }

        for name in duplicate_names {
            bindings.remove(&name);
        }

        let mut assignments = AssignedBindingCollector::new(bindings.keys().cloned().collect());
        assignments.visit_program(program);

        Self {
            source,
            bindings,
            resolved_strings: HashMap::new(),
            assigned_names: assignments.names,
        }
    }

    fn resolve_all(&mut self) {
        self.resolved_strings = self
            .bindings
            .iter()
            .filter_map(|(name, init)| {
                if self.assigned_names.contains(name) {
                    return None;
                }

                let mut resolving = HashSet::from([name.clone()]);
                let mut visits = 0;
                let content_span = self.resolve_expression(init, 0, &mut visits, &mut resolving)?;
                Some((
                    name.clone(),
                    ResolvedStaticString {
                        content_span,
                        available_after: init.span().end,
                    },
                ))
            })
            .collect();
    }

    fn resolve_expression(
        &self,
        expression: &'a Expression<'a>,
        depth: usize,
        visits: &mut usize,
        resolving: &mut HashSet<String>,
    ) -> Option<Span> {
        if depth >= MAX_STATIC_STRING_DEPTH || *visits >= MAX_STATIC_STRING_VISITS {
            return None;
        }
        *visits += 1;

        let expression = unwrap_expression(expression);
        if let Some(span) = literal_static_string_span(expression, self.source) {
            return Some(span);
        }

        let Expression::Identifier(identifier) = expression else {
            return None;
        };
        let name = identifier.name.as_str();
        if self.assigned_names.contains(name) || !resolving.insert(name.to_string()) {
            return None;
        }

        let init = self.bindings.get(name)?;
        if init.span().end > identifier.span.end {
            resolving.remove(name);
            return None;
        }

        let result = self.resolve_expression(init, depth + 1, visits, resolving);
        resolving.remove(name);
        result
    }
}

fn collect_static_resolver<'a, 's>(
    program: &'a Program<'a>,
    source: &'s str,
) -> StaticStringResolver<'a, 's> {
    let mut resolver = StaticStringResolver::from_program(program, source);
    resolver.resolve_all();
    resolver
}

struct DefineConfigImportCollector<'s> {
    module_source: &'s str,
    define_config_bindings: HashSet<String>,
    define_config_namespaces: HashSet<String>,
}

impl<'s> DefineConfigImportCollector<'s> {
    fn new(module_source: &'s str) -> Self {
        Self {
            module_source,
            define_config_bindings: HashSet::new(),
            define_config_namespaces: HashSet::new(),
        }
    }

    fn collect_commonjs_program(&mut self, program: &Program<'_>) {
        for statement in &program.body {
            let Statement::VariableDeclaration(declaration) = statement else {
                continue;
            };
            if !declaration.kind.is_const() {
                continue;
            }
            for declarator in &declaration.declarations {
                self.collect_commonjs_declarator(declarator);
            }
        }
    }

    fn reject_mutated_helpers(&mut self, program: &Program<'_>) {
        let tracked_names = self
            .define_config_bindings
            .iter()
            .chain(&self.define_config_namespaces)
            .cloned()
            .collect();
        let mut assignments =
            AssignedBindingCollector::new(tracked_names).with_member_property("defineConfig");
        assignments.visit_program(program);
        self.define_config_bindings
            .retain(|name| !assignments.names.contains(name));
        self.define_config_namespaces
            .retain(|name| !assignments.names.contains(name));
    }

    fn collect_commonjs_declarator(&mut self, declarator: &VariableDeclarator<'_>) {
        let Some(init) = declarator.init.as_ref().map(unwrap_expression) else {
            return;
        };

        if let Some(member) = init.as_member_expression() {
            if member.static_property_name() == Some("defineConfig")
                && is_require_expression(member.object(), self.module_source)
            {
                if let BindingPattern::BindingIdentifier(local) = &declarator.id {
                    self.define_config_bindings
                        .insert(local.name.as_str().to_string());
                }
            }
            return;
        }

        if !is_require_expression(init, self.module_source) {
            return;
        }

        match &declarator.id {
            BindingPattern::BindingIdentifier(local) => {
                self.define_config_namespaces
                    .insert(local.name.as_str().to_string());
            }
            BindingPattern::ObjectPattern(pattern) => {
                for property in &pattern.properties {
                    if property.computed || !property_key_matches(&property.key, "defineConfig") {
                        continue;
                    }
                    if let BindingPattern::BindingIdentifier(local) = &property.value {
                        self.define_config_bindings
                            .insert(local.name.as_str().to_string());
                    }
                }
            }
            _ => {}
        }
    }
}

impl<'a> Visit<'a> for DefineConfigImportCollector<'_> {
    fn visit_import_declaration(&mut self, declaration: &ImportDeclaration<'a>) {
        if declaration.import_kind.is_type()
            || declaration.source.value.as_str() != self.module_source
        {
            return;
        }
        let Some(specifiers) = declaration.specifiers.as_ref() else {
            return;
        };

        for specifier in specifiers {
            match specifier {
                ImportDeclarationSpecifier::ImportSpecifier(specifier)
                    if specifier.import_kind.is_value()
                        && specifier.imported.name().as_str() == "defineConfig" =>
                {
                    self.define_config_bindings
                        .insert(specifier.local.name.as_str().to_string());
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => {
                    self.define_config_namespaces
                        .insert(specifier.local.name.as_str().to_string());
                }
                _ => {}
            }
        }
    }
}

struct AstroFontExtractor<'a, 's> {
    source: &'s str,
    define_config_bindings: HashSet<String>,
    define_config_namespaces: HashSet<String>,
    expression_resolver: StaticExpressionResolver<'a>,
    variables: Vec<ExtractedVariable>,
}

impl<'a, 's> AstroFontExtractor<'a, 's> {
    fn extract_esm_program(&mut self, program: &'a Program<'a>) {
        for statement in &program.body {
            if let Statement::ExportDefaultDeclaration(declaration) = statement {
                if let Some(expression) = declaration.declaration.as_expression() {
                    self.extract_default_expression(expression);
                }
            }
        }
    }

    fn extract_default_expression(&mut self, expression: &'a Expression<'a>) {
        if let Some(config) = config_object_from_expression(
            expression,
            &self.define_config_bindings,
            &self.define_config_namespaces,
            &self.expression_resolver,
        ) {
            self.extract_astro_config(config);
        }
    }

    fn extract_commonjs_program(&mut self, program: &'a Program<'a>) {
        if let Some(config) = commonjs_config_object(
            program,
            self.source,
            &self.define_config_bindings,
            &self.define_config_namespaces,
            &self.expression_resolver,
        ) {
            self.extract_astro_config(config);
        }
    }

    fn extract_astro_config(&mut self, config: &'a ObjectExpression<'a>) {
        if let Some(fonts) = self
            .expression_resolver
            .property_value(config, "fonts")
            .and_then(|value| self.expression_resolver.as_array(value))
        {
            self.extract_fonts(fonts);
        }

        if let Some(experimental) = self
            .expression_resolver
            .property_value(config, "experimental")
            .and_then(|value| self.expression_resolver.as_object(value))
        {
            if let Some(fonts) = self
                .expression_resolver
                .property_value(experimental, "fonts")
                .and_then(|value| self.expression_resolver.as_array(value))
            {
                self.extract_fonts(fonts);
            }
        }
    }

    fn extract_fonts(&mut self, fonts: &'a ArrayExpression<'a>) {
        let mut active_arrays = HashSet::new();
        let mut visits = 0;
        if !self.array_is_fully_static(fonts, 0, &mut visits, &mut active_arrays) {
            return;
        }
        self.extract_known_fonts(fonts);
    }

    fn array_is_fully_static(
        &self,
        array: &'a ArrayExpression<'a>,
        depth: usize,
        visits: &mut usize,
        active_arrays: &mut HashSet<(u32, u32)>,
    ) -> bool {
        if depth >= MAX_STATIC_STRING_DEPTH || *visits >= MAX_STATIC_STRUCTURE_VISITS {
            return false;
        }
        *visits += 1;
        let key = (array.span.start, array.span.end);
        if !active_arrays.insert(key) {
            return false;
        }

        let mut is_static = true;
        for element in &array.elements {
            let ArrayExpressionElement::SpreadElement(spread) = element else {
                continue;
            };
            let Some(spread_array) = self.expression_resolver.as_array(&spread.argument) else {
                is_static = false;
                break;
            };
            if !self.array_is_fully_static(spread_array, depth + 1, visits, active_arrays) {
                is_static = false;
                break;
            }
        }

        active_arrays.remove(&key);
        is_static
    }

    fn extract_known_fonts(&mut self, fonts: &'a ArrayExpression<'a>) {
        for element in &fonts.elements {
            if let ArrayExpressionElement::SpreadElement(spread) = element {
                if let Some(array) = self.expression_resolver.as_array(&spread.argument) {
                    self.extract_known_fonts(array);
                }
                continue;
            }
            let Some(font) = self.array_element_object(element) else {
                continue;
            };
            let Some(ResolvedProperty::Known {
                value,
                declaration_span,
            }) = self.expression_resolver.property(font, "cssVariable")
            else {
                continue;
            };
            let Some((name, name_span)) =
                static_string_value(value, self.source, self.expression_resolver.strings)
            else {
                continue;
            };
            if !is_supported_custom_property_name(&name) {
                continue;
            }
            if self
                .variables
                .iter()
                .any(|variable| variable.name == name && variable.name_span == name_span)
            {
                continue;
            }

            self.variables.push(ExtractedVariable {
                name,
                declaration_span: if matches!(unwrap_expression(value), Expression::Identifier(_)) {
                    name_span
                } else {
                    declaration_span
                },
                name_span,
            });
        }
    }

    fn array_element_object(
        &self,
        element: &'a ArrayExpressionElement<'a>,
    ) -> Option<&'a ObjectExpression<'a>> {
        let expression = element.as_expression()?;
        self.expression_resolver.as_object(expression)
    }
}

struct ViteAdditionalDataExtractor<'a, 's> {
    source: &'s str,
    define_config_bindings: HashSet<String>,
    define_config_namespaces: HashSet<String>,
    expression_resolver: StaticExpressionResolver<'a>,
    css_snippets: Vec<ExtractedCssSnippet>,
}

impl<'a, 's> ViteAdditionalDataExtractor<'a, 's> {
    fn extract_esm_program(&mut self, program: &'a Program<'a>) {
        for statement in &program.body {
            if let Statement::ExportDefaultDeclaration(declaration) = statement {
                if let Some(expression) = declaration.declaration.as_expression() {
                    self.extract_default_expression(expression);
                }
            }
        }
    }

    fn extract_default_expression(&mut self, expression: &'a Expression<'a>) {
        if let Some(config) = config_object_from_expression(
            expression,
            &self.define_config_bindings,
            &self.define_config_namespaces,
            &self.expression_resolver,
        ) {
            self.extract_vite_config(config);
        }
    }

    fn extract_commonjs_program(&mut self, program: &'a Program<'a>) {
        if let Some(config) = commonjs_config_object(
            program,
            self.source,
            &self.define_config_bindings,
            &self.define_config_namespaces,
            &self.expression_resolver,
        ) {
            self.extract_vite_config(config);
        }
    }

    fn extract_vite_config(&mut self, config: &'a ObjectExpression<'a>) {
        let Some(preprocessor_options) = self
            .expression_resolver
            .property_value(config, "css")
            .and_then(|value| self.expression_resolver.as_object(value))
            .and_then(|css| {
                self.expression_resolver
                    .property_value(css, "preprocessorOptions")
            })
            .and_then(|value| self.expression_resolver.as_object(value))
        else {
            return;
        };

        let Some(options) = self
            .expression_resolver
            .property_value(preprocessor_options, VITE_PREPROCESSOR)
            .and_then(|value| self.expression_resolver.as_object(value))
        else {
            return;
        };
        let Some(additional_data) = self
            .expression_resolver
            .property_value(options, "additionalData")
        else {
            return;
        };
        let Some((text, content_span)) = static_string_value(
            additional_data,
            self.source,
            self.expression_resolver.strings,
        ) else {
            return;
        };
        if contains_unsupported_scss_control_flow(&text) {
            return;
        }
        self.css_snippets
            .push(ExtractedCssSnippet { text, content_span });
    }
}

fn config_object_from_expression<'a>(
    expression: &'a Expression<'a>,
    define_config_bindings: &HashSet<String>,
    define_config_namespaces: &HashSet<String>,
    resolver: &StaticExpressionResolver<'a>,
) -> Option<&'a ObjectExpression<'a>> {
    match unwrap_expression(expression) {
        Expression::ObjectExpression(object) => Some(object.as_ref()),
        Expression::Identifier(_) => resolver.as_object(expression),
        Expression::ArrowFunctionExpression(function) => {
            config_object_from_arrow_body(&function.body, resolver)
        }
        Expression::FunctionExpression(function) => function
            .body
            .as_deref()
            .and_then(|body| config_object_from_function_body(body, resolver)),
        Expression::CallExpression(call)
            if is_define_config_callee(
                &call.callee,
                define_config_bindings,
                define_config_namespaces,
            ) =>
        {
            call.arguments
                .first()
                .and_then(|argument| argument.as_expression())
                .and_then(|expression| {
                    config_object_from_expression(
                        expression,
                        define_config_bindings,
                        define_config_namespaces,
                        resolver,
                    )
                })
        }
        _ => None,
    }
}

fn config_object_from_arrow_body<'a>(
    body: &'a ArrowFunctionBody<'a>,
    resolver: &StaticExpressionResolver<'a>,
) -> Option<&'a ObjectExpression<'a>> {
    if let Some(expression) = body.as_expression() {
        resolver.as_object(expression)
    } else if let ArrowFunctionBody::FunctionBody(body) = body {
        config_object_from_function_body(body, resolver)
    } else {
        None
    }
}

fn config_object_from_function_body<'a>(
    body: &'a oxc_ast::ast::FunctionBody<'a>,
    resolver: &StaticExpressionResolver<'a>,
) -> Option<&'a ObjectExpression<'a>> {
    let [Statement::ReturnStatement(statement)] = body.statements.as_slice() else {
        return None;
    };
    statement
        .argument
        .as_ref()
        .and_then(|expression| resolver.as_object(expression))
}

fn commonjs_config_object<'a>(
    program: &'a Program<'a>,
    source: &str,
    define_config_bindings: &HashSet<String>,
    define_config_namespaces: &HashSet<String>,
    resolver: &StaticExpressionResolver<'a>,
) -> Option<&'a ObjectExpression<'a>> {
    let mut assignments = ModuleExportsAssignmentCollector::new(source);
    assignments.visit_program(program);

    for statement in program.body.iter().rev() {
        let Statement::ExpressionStatement(statement) = statement else {
            continue;
        };
        let Expression::AssignmentExpression(assignment) = unwrap_expression(&statement.expression)
        else {
            continue;
        };
        if is_module_exports_assignment(assignment, source) {
            if assignments
                .spans
                .iter()
                .any(|span| span.start > assignment.span.start)
            {
                return None;
            }
            return config_object_from_expression(
                &assignment.right,
                define_config_bindings,
                define_config_namespaces,
                resolver,
            );
        }
    }
    None
}

struct ModuleExportsAssignmentCollector<'s> {
    source: &'s str,
    spans: Vec<Span>,
}

impl<'s> ModuleExportsAssignmentCollector<'s> {
    fn new(source: &'s str) -> Self {
        Self {
            source,
            spans: Vec::new(),
        }
    }
}

impl<'a> Visit<'a> for ModuleExportsAssignmentCollector<'_> {
    fn visit_assignment_expression(&mut self, assignment: &AssignmentExpression<'a>) {
        if is_module_exports_assignment(assignment, self.source) {
            self.spans.push(assignment.span);
        }
        walk_assignment_expression(self, assignment);
    }

    fn visit_function_body(&mut self, _body: &oxc_ast::ast::FunctionBody<'a>) {}
}

fn is_require_expression(expression: &Expression<'_>, module_source: &str) -> bool {
    let Expression::CallExpression(call) = unwrap_expression(expression) else {
        return false;
    };
    call.is_require_call()
        && matches!(
            call.arguments.first(),
            Some(Argument::StringLiteral(source)) if source.value.as_str() == module_source
        )
}

fn is_define_config_callee(
    callee: &Expression<'_>,
    define_config_bindings: &HashSet<String>,
    define_config_namespaces: &HashSet<String>,
) -> bool {
    let callee = unwrap_expression(callee);
    if matches!(
        callee,
        Expression::Identifier(identifier)
            if define_config_bindings.contains(identifier.name.as_str())
    ) {
        return true;
    }

    let Some(member) = callee.as_member_expression() else {
        return false;
    };
    if member.static_property_name() != Some("defineConfig") {
        return false;
    }
    matches!(
        unwrap_expression(member.object()),
        Expression::Identifier(identifier)
            if define_config_namespaces.contains(identifier.name.as_str())
    )
}

fn is_module_exports_assignment(assignment: &AssignmentExpression<'_>, source: &str) -> bool {
    assignment.operator.is_assign()
        && source
            .get(assignment.left.span().start as usize..assignment.left.span().end as usize)
            .is_some_and(|target| target.trim() == "module.exports")
}

fn unwrap_expression<'a>(mut expression: &'a Expression<'a>) -> &'a Expression<'a> {
    loop {
        expression = match expression {
            Expression::ParenthesizedExpression(wrapper) => &wrapper.expression,
            Expression::TSAsExpression(wrapper) => &wrapper.expression,
            Expression::TSSatisfiesExpression(wrapper) => &wrapper.expression,
            Expression::TSTypeAssertion(wrapper) => &wrapper.expression,
            Expression::TSNonNullExpression(wrapper) => &wrapper.expression,
            Expression::TSInstantiationExpression(wrapper) => &wrapper.expression,
            _ => return expression,
        };
    }
}

fn property_key_matches(key: &PropertyKey<'_>, name: &str) -> bool {
    key.is_specific_id(name) || key.is_specific_string_literal(name)
}

fn is_supported_custom_property_name(name: &str) -> bool {
    let Some(suffix) = name.strip_prefix("--") else {
        return false;
    };
    !suffix.is_empty()
        && suffix
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '-' | '_'))
}

fn as_object_expression<'a>(expression: &'a Expression<'a>) -> Option<&'a ObjectExpression<'a>> {
    match expression {
        Expression::ObjectExpression(object) => Some(object.as_ref()),
        _ => None,
    }
}

fn literal_content_span(span: Span) -> Span {
    if span.end > span.start + 1 {
        Span::new(span.start + 1, span.end - 1)
    } else {
        span
    }
}

fn literal_static_string_span(expression: &Expression<'_>, source: &str) -> Option<Span> {
    match expression {
        Expression::StringLiteral(literal) => {
            let span = literal_content_span(literal.span);
            let raw = source.get(span.start as usize..span.end as usize)?;
            (raw == literal.value.as_str()).then_some(span)
        }
        Expression::TemplateLiteral(template)
            if template.expressions.is_empty() && template.quasis.len() == 1 =>
        {
            let span = literal_content_span(template.span);
            let raw = source.get(span.start as usize..span.end as usize)?;
            (!raw.contains('\\')).then_some(span)
        }
        _ => None,
    }
}

fn static_string_value(
    expression: &Expression<'_>,
    source: &str,
    static_strings: &HashMap<String, ResolvedStaticString>,
) -> Option<(String, Span)> {
    let expression = unwrap_expression(expression);
    let content_span = if let Some(span) = literal_static_string_span(expression, source) {
        span
    } else {
        let Expression::Identifier(identifier) = expression else {
            return None;
        };
        let value = static_strings.get(identifier.name.as_str())?;
        if value.available_after > identifier.span.start {
            return None;
        }
        value.content_span
    };
    let value = source.get(content_span.start as usize..content_span.end as usize)?;
    Some((value.to_string(), content_span))
}

fn contains_unsupported_scss_control_flow(source: &str) -> bool {
    const UNSUPPORTED_AT_RULES: &[&str] = &[
        "if", "else", "for", "each", "while", "mixin", "include", "content", "function", "return",
        "extend", "at-root",
    ];

    let bytes = source.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'@' {
            index += 1;
            continue;
        }

        let start = index + 1;
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'-') {
            end += 1;
        }
        if end > start {
            let name = &source[start..end];
            if UNSUPPORTED_AT_RULES
                .iter()
                .any(|candidate| name.eq_ignore_ascii_case(candidate))
            {
                return true;
            }
        }
        index = end.max(index + 1);
    }

    false
}

fn span_to_range(text: &str, span: Span) -> Range {
    Range::new(
        offset_to_position(text, span.start as usize),
        offset_to_position(text, span.end as usize),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Config;

    fn test_uri(name: &str) -> Uri {
        Uri::from_file_path(std::env::temp_dir().join(name)).unwrap()
    }

    #[test]
    fn recognizes_supported_astro_config_names() {
        assert!(is_supported_config_path(Path::new("astro.config.mjs")));
        assert!(is_supported_config_path(Path::new(
            "/workspace/astro.config.ts"
        )));
        assert!(!is_supported_config_path(Path::new("astro.config.json")));
        assert!(!is_supported_config_path(Path::new("astro.config.foo.ts")));
        assert!(!is_supported_config_path(Path::new("src/config.ts")));
    }

    #[test]
    fn recognizes_supported_vite_config_names() {
        for name in [
            "vite.config.js",
            "vite.config.mjs",
            "vite.config.cjs",
            "vite.config.ts",
            "vite.config.mts",
            "vite.config.cts",
        ] {
            assert!(is_supported_config_path(Path::new(name)), "{name}");
        }
        assert!(!is_supported_config_path(Path::new("vite.config.json")));
        assert!(!is_supported_config_path(Path::new("src/vite.ts")));
    }

    #[tokio::test]
    async fn extracts_vite_preprocessor_additional_data_variables() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("vite.config.ts");
        let text = r#"
            import { defineConfig as configure } from "vite";

            export default configure({
                css: {
                    preprocessorOptions: {
                        scss: {
                            additionalData: `:root {
                                --vite-brand: #123456;
                                --vite-derived: var(--base-color);
                            }`,
                        },
                        less: {
                            additionalData: ".theme { --vite-less: rebeccapurple; }",
                        },
                    },
                },
            });
        "#;

        parse_config_document(text, &uri, &manager).await.unwrap();

        let brand = manager.get_variables("--vite-brand").await;
        assert_eq!(brand.len(), 1);
        assert_eq!(brand[0].value, "#123456");
        assert_eq!(brand[0].selector, ":root");
        let brand_range = brand[0].name_range.expect("name range");
        let brand_start = text.find("--vite-brand").unwrap();
        assert_eq!(
            brand_range,
            Range::new(
                offset_to_position(text, brand_start),
                offset_to_position(text, brand_start + "--vite-brand".len()),
            )
        );

        assert_eq!(manager.get_variables("--vite-derived").await.len(), 1);
        assert_eq!(manager.get_usages("--base-color").await.len(), 1);
        assert!(manager.get_variables("--vite-less").await.is_empty());
    }

    #[tokio::test]
    async fn resolves_reusable_top_level_const_strings_for_astro_and_vite() {
        let astro_manager = CssVariableManager::new(Config::default());
        let astro_uri = test_uri("astro.config.ts");
        let astro_text = r#"
            import { defineConfig } from "astro/config";

            const FONT_NAME = ("--font-const" as const) satisfies string;
            const FONT_ALIAS = FONT_NAME;

            export default defineConfig({
                fonts: [
                    { cssVariable: FONT_ALIAS },
                    { cssVariable: FONT_ALIAS },
                ],
            });
        "#;

        parse_config_document(astro_text, &astro_uri, &astro_manager)
            .await
            .unwrap();

        let font = astro_manager.get_variables("--font-const").await;
        assert_eq!(font.len(), 1);
        let font_start = astro_text.find("--font-const").unwrap();
        assert_eq!(
            font[0].name_range,
            Some(Range::new(
                offset_to_position(astro_text, font_start),
                offset_to_position(astro_text, font_start + "--font-const".len()),
            ))
        );

        let vite_manager = CssVariableManager::new(Config::default());
        let vite_uri = test_uri("vite.config.ts");
        let vite_text = r#"
            import { defineConfig } from "vite";

            const SHARED_SCSS = `:root {
                --vite-const: #123456;
                --vite-const-derived: var(--base-color);
            }`;

            export default defineConfig({
                css: {
                    preprocessorOptions: {
                        scss: { additionalData: SHARED_SCSS },
                    },
                },
            });
        "#;

        parse_config_document(vite_text, &vite_uri, &vite_manager)
            .await
            .unwrap();

        assert_eq!(vite_manager.get_variables("--vite-const").await.len(), 1);
        let vite_const = vite_manager.get_variables("--vite-const").await;
        let vite_start = vite_text.find("--vite-const").unwrap();
        assert_eq!(
            vite_const[0].name_range,
            Some(Range::new(
                offset_to_position(vite_text, vite_start),
                offset_to_position(vite_text, vite_start + "--vite-const".len()),
            ))
        );
        assert_eq!(
            vite_manager
                .get_variables("--vite-const-derived")
                .await
                .len(),
            1
        );
        assert_eq!(vite_manager.get_usages("--base-color").await.len(), 1);

        let commonjs_manager = CssVariableManager::new(Config::default());
        let commonjs_uri = test_uri("astro.config.cjs");
        parse_config_document(
            r#"
                const FONT_NAME = "--font-commonjs-const";
                module.exports = {
                    fonts: [{ cssVariable: FONT_NAME }],
                };
            "#,
            &commonjs_uri,
            &commonjs_manager,
        )
        .await
        .unwrap();
        assert_eq!(
            commonjs_manager
                .get_variables("--font-commonjs-const")
                .await
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn const_string_resolution_rejects_dynamic_or_unsafe_bindings() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("astro.config.ts");
        parse_config_document(
            r#"
                import { defineConfig } from "astro/config";

                let LET_NAME = "--font-let";
                var VAR_NAME = "--font-var";
                const DYNAMIC = `--font-${family}`;

                export default defineConfig({
                    fonts: [
                        { cssVariable: LET_NAME },
                        { cssVariable: VAR_NAME },
                        { cssVariable: DYNAMIC },
                        { cssVariable: DECLARED_AFTER_USE },
                        { cssVariable: LOCAL_ONLY },
                    ],
                });

                const DECLARED_AFTER_USE = "--font-after-use";
                function unused() {
                    const LOCAL_ONLY = "--font-local";
                    return LOCAL_ONLY;
                }
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        for name in [
            "--font-let",
            "--font-var",
            "--font-after-use",
            "--font-local",
        ] {
            assert!(manager.get_variables(name).await.is_empty(), "{name}");
        }
    }

    #[test]
    fn static_string_resolution_rejects_mutation_cycles_and_excessive_depth() {
        let mut text = String::from(
            r#"
                const REASSIGNED = "--font-before-reassignment";
                REASSIGNED = "--font-after-reassignment";
                const UPDATED = "--font-before-update";
                UPDATED++;
                const DESTRUCTURED = "--font-before-destructure";
                ({ value: DESTRUCTURED } = source);
                const FOR_OF_TARGET = "--font-before-for-of";
                for (FOR_OF_TARGET of values) {}
                const TS_WRAPPED = "--font-before-ts-assignment";
                (TS_WRAPPED as string) = source;
                const SHADOWED = "--font-shadowed";
                {
                    let SHADOWED = 0;
                    SHADOWED++;
                }
                const SWITCH_SHADOWED = "--font-switch-shadowed";
                switch (mode) {
                    case 1:
                        let SWITCH_SHADOWED;
                        SWITCH_SHADOWED = source;
                        break;
                }
                const CYCLE_A = CYCLE_B;
                const CYCLE_B = CYCLE_A;
                const VALUE_20 = "--font-depth";
            "#,
        );
        for index in (0..20).rev() {
            text.push_str(&format!("const VALUE_{index} = VALUE_{};\n", index + 1));
        }

        let allocator = Allocator::default();
        let source_type = SourceType::from_path(Path::new("astro.config.ts")).unwrap();
        let parsed = Parser::new(&allocator, &text, source_type).parse();
        let values = collect_static_resolver(&parsed.program, &text).resolved_strings;

        assert!(!values.contains_key("REASSIGNED"));
        assert!(!values.contains_key("UPDATED"));
        assert!(!values.contains_key("DESTRUCTURED"));
        assert!(!values.contains_key("FOR_OF_TARGET"));
        assert!(!values.contains_key("TS_WRAPPED"));
        assert!(values.contains_key("SHADOWED"));
        assert!(values.contains_key("SWITCH_SHADOWED"));
        assert!(!values.contains_key("CYCLE_A"));
        assert!(!values.contains_key("CYCLE_B"));
        assert!(!values.contains_key("VALUE_0"));
        let depth_span = values.get("VALUE_5").unwrap().content_span;
        assert_eq!(
            text.get(depth_span.start as usize..depth_span.end as usize),
            Some("--font-depth")
        );
    }

    #[tokio::test]
    async fn exported_const_strings_remain_out_of_scope() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("astro.config.ts");
        parse_config_document(
            r#"
                export const FONT_NAME = "--font-exported-const";
                export default {
                    fonts: [{ cssVariable: FONT_NAME }],
                };
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert!(manager
            .get_variables("--font-exported-const")
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn resolves_static_config_structures_and_vite_function_returns() {
        let astro_manager = CssVariableManager::new(Config::default());
        let astro_uri = test_uri("astro.config.ts");
        parse_config_document(
            r#"
                import { defineConfig } from "astro/config";

                const BODY_FONT = { cssVariable: "--font-structured-body" };
                const HEADING_FONT = { cssVariable: "--font-structured-heading" };
                const BASE_FONTS = [BODY_FONT];
                const FONTS = [...BASE_FONTS, HEADING_FONT];
                const FONT_CONFIG = { fonts: FONTS };
                const CONFIG = { ...FONT_CONFIG };

                export default defineConfig(CONFIG);
            "#,
            &astro_uri,
            &astro_manager,
        )
        .await
        .unwrap();

        assert_eq!(
            astro_manager
                .get_variables("--font-structured-body")
                .await
                .len(),
            1
        );
        assert_eq!(
            astro_manager
                .get_variables("--font-structured-heading")
                .await
                .len(),
            1
        );

        let vite_manager = CssVariableManager::new(Config::default());
        let vite_uri = test_uri("vite.config.ts");
        parse_config_document(
            r#"
                import { defineConfig } from "vite";

                const SHARED_SCSS = `:root { --vite-structured: #123456; }`;
                const SCSS = { additionalData: SHARED_SCSS };
                const PREPROCESSORS = { scss: SCSS };
                const CSS = { preprocessorOptions: PREPROCESSORS };
                const BASE = { css: CSS };

                export default defineConfig(() => ({ ...BASE }));
            "#,
            &vite_uri,
            &vite_manager,
        )
        .await
        .unwrap();

        assert_eq!(
            vite_manager.get_variables("--vite-structured").await.len(),
            1
        );
    }

    #[tokio::test]
    async fn static_structure_resolution_rejects_unknown_overrides_and_dynamic_functions() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("vite.config.ts");
        parse_config_document(
            r#"
                import { defineConfig } from "vite";

                const VALID = {
                    css: {
                        preprocessorOptions: {
                            scss: { additionalData: ":root { --vite-before-unknown: red; }" },
                        },
                    },
                    ...runtimeConfig,
                };

                export default defineConfig((env) => {
                    if (env.mode === "production") {
                        return VALID;
                    }
                    return {
                        css: {
                            preprocessorOptions: {
                                scss: { additionalData: ":root { --vite-dynamic-return: blue; }" },
                            },
                        },
                    };
                });
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert!(manager
            .get_variables("--vite-before-unknown")
            .await
            .is_empty());
        assert!(manager
            .get_variables("--vite-dynamic-return")
            .await
            .is_empty());

        let computed_manager = CssVariableManager::new(Config::default());
        parse_config_document(
            r#"
                import { defineConfig } from "vite";

                const CSS = {
                    preprocessorOptions: {
                        scss: {
                            additionalData: ":root { --vite-before-computed: green; }",
                            [runtimeKey]: runtimeValue,
                        },
                    },
                };

                export default defineConfig({
                    css: CSS,
                    [runtimeKey]: runtimeValue,
                });
            "#,
            &uri,
            &computed_manager,
        )
        .await
        .unwrap();
        assert!(computed_manager
            .get_variables("--vite-before-computed")
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn static_structure_resolution_rejects_direct_member_mutation() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("astro.config.ts");
        parse_config_document(
            r#"
                const CONFIG = {
                    fonts: [{ cssVariable: "--font-before-member-write" }],
                };
                CONFIG.fonts = [{ cssVariable: "--font-after-member-write" }];
                export default CONFIG;
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert!(manager
            .get_variables("--font-before-member-write")
            .await
            .is_empty());
        assert!(manager
            .get_variables("--font-after-member-write")
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn cyclic_object_spreads_are_rejected_without_unbounded_recursion() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("astro.config.ts");
        parse_config_document(
            r#"
                const CONFIG = {
                    fonts: [{ cssVariable: "--font-object-cycle" }],
                    ...CONFIG,
                };
                export default CONFIG;
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert!(manager
            .get_variables("--font-object-cycle")
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn cyclic_and_unknown_array_spreads_reject_the_entire_fonts_value() {
        for (name, spread) in [
            ("--font-array-cycle", "...FONTS"),
            ("--font-array-unknown", "...runtimeFonts"),
        ] {
            let manager = CssVariableManager::new(Config::default());
            let uri = test_uri("astro.config.ts");
            let text = format!(
                r#"
                    const FONTS = [{{ cssVariable: "{name}" }}, {spread}];
                    export default {{ fonts: FONTS }};
                "#
            );
            parse_config_document(&text, &uri, &manager).await.unwrap();
            assert!(manager.get_variables(name).await.is_empty(), "{spread}");
        }
    }

    #[tokio::test]
    async fn vite_extraction_accepts_commonjs_and_rejects_dynamic_or_unrelated_values() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("vite.config.cjs");
        parse_config_document(
            r#"
                const { defineConfig: configure } = require("vite");
                module.exports = configure({
                    define: {
                        __BRAND_VARIABLE__: JSON.stringify("--vite-define"),
                    },
                    additionalData: ":root { --wrong-level: red; }",
                    css: {
                        preprocessorOptions: {
                            scss: {
                                additionalData: () => ":root { --dynamic: red; }",
                            },
                            scss: {
                                additionalData: ":root { --vite-cjs: blue; }",
                            },
                        },
                    },
                });
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert_eq!(manager.get_variables("--vite-cjs").await.len(), 1);
        assert!(manager.get_variables("--vite-define").await.is_empty());
        assert!(manager.get_variables("--wrong-level").await.is_empty());
        assert!(manager.get_variables("--dynamic").await.is_empty());
    }

    #[tokio::test]
    async fn vite_extraction_requires_a_proven_define_config_binding() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("vite.config.ts");
        parse_config_document(
            r#"
                const defineConfig = (value) => value;
                export default defineConfig({
                    css: {
                        preprocessorOptions: {
                            scss: {
                                additionalData: ":root { --unproven-vite: red; }",
                            },
                        },
                    },
                });
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert!(manager.get_variables("--unproven-vite").await.is_empty());
    }

    #[tokio::test]
    async fn vite_extraction_rejects_type_only_define_config_imports() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("vite.config.ts");
        parse_config_document(
            r#"
                import type { defineConfig } from "vite";
                export default defineConfig({
                    css: {
                        preprocessorOptions: {
                            scss: {
                                additionalData: ":root { --type-only-vite: red; }",
                            },
                        },
                    },
                });
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert!(manager.get_variables("--type-only-vite").await.is_empty());
    }

    #[tokio::test]
    async fn vite_extraction_prefers_esm_over_commonjs_in_ambiguous_sources() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("vite.config.ts");
        parse_config_document(
            r#"
                import { defineConfig } from "vite";
                export default defineConfig({
                    css: {
                        preprocessorOptions: {
                            scss: {
                                additionalData: ":root { --vite-esm: red; }",
                            },
                        },
                    },
                });
                module.exports = {
                    css: {
                        preprocessorOptions: {
                            scss: {
                                additionalData: ":root { --vite-cjs-shadow: blue; }",
                            },
                        },
                    },
                };
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert_eq!(manager.get_variables("--vite-esm").await.len(), 1);
        assert!(manager.get_variables("--vite-cjs-shadow").await.is_empty());
    }

    #[tokio::test]
    async fn vite_extraction_preserves_crlf_template_ranges() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("vite.config.ts");
        let text = "import { defineConfig } from \"vite\";\r\nexport default defineConfig({ css: { preprocessorOptions: { scss: { additionalData: `:root {\r\n  --vite-crlf: red;\r\n}` } } } });\r\n";

        parse_config_document(text, &uri, &manager).await.unwrap();

        let variable = manager.get_variables("--vite-crlf").await;
        assert_eq!(variable.len(), 1);
        let start = text.find("--vite-crlf").unwrap();
        assert_eq!(
            variable[0].name_range,
            Some(Range::new(
                offset_to_position(text, start),
                offset_to_position(text, start + "--vite-crlf".len()),
            ))
        );
    }

    #[tokio::test]
    async fn vite_extraction_rejects_scss_control_flow_conservatively() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("vite.config.ts");
        parse_config_document(
            r#"
                export default {
                    css: {
                        preprocessorOptions: {
                            scss: {
                                additionalData: `
                                    @if false {
                                        :root { --vite-phantom: red; }
                                    }
                                    :root { --vite-after-conditional: blue; }
                                `,
                            },
                        },
                    },
                };
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert!(manager.get_variables("--vite-phantom").await.is_empty());
        assert!(manager
            .get_variables("--vite-after-conditional")
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn extracts_static_font_variables_from_current_and_legacy_locations() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("astro.config.ts");
        let text = r#"
            import { defineConfig as astroConfig } from "astro/config";
            let dynamicName = "--font-dynamic";
            const unrelated = { fonts: [{ cssVariable: "--font-unrelated" }] };
            export default astroConfig({
                fonts: [
                    { cssVariable: "--font-body" },
                    { "cssVariable": '--font-heading' },
                    { cssVariable: `--font-code` },
                    { cssVariable: `--font-${family}` },
                    { cssVariable: dynamicName },
                ],
                experimental: {
                    fonts: [{ cssVariable: "--font-legacy" }],
                },
            });
        "#;

        parse_config_document(text, &uri, &manager).await.unwrap();

        assert_eq!(manager.get_variables("--font-body").await.len(), 1);
        assert_eq!(manager.get_variables("--font-heading").await.len(), 1);
        assert_eq!(manager.get_variables("--font-code").await.len(), 1);
        assert_eq!(manager.get_variables("--font-legacy").await.len(), 1);
        assert!(manager.get_variables("--font-dynamic").await.is_empty());
        assert!(manager.get_variables("--font-unrelated").await.is_empty());
    }

    #[tokio::test]
    async fn extracts_from_a_direct_default_object() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("astro.config.mjs");
        let text = r#"export default { fonts: [{ cssVariable: "--font-direct" }] };"#;

        parse_config_document(text, &uri, &manager).await.unwrap();

        let variables = manager.get_variables("--font-direct").await;
        assert_eq!(variables.len(), 1);
        let declaration_start = text.find("cssVariable").unwrap();
        let declaration_end = text.find("\"--font-direct\"").unwrap() + "\"--font-direct\"".len();
        assert_eq!(
            variables[0].range,
            Range::new(
                offset_to_position(text, declaration_start),
                offset_to_position(text, declaration_end),
            ),
            "direct literals should retain the containing property declaration range"
        );
    }

    #[tokio::test]
    async fn extracts_from_commonjs_configs() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("astro.config.cjs");

        parse_config_document(
            r#"module.exports = { fonts: [{ cssVariable: "--font-commonjs" }] };"#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert_eq!(manager.get_variables("--font-commonjs").await.len(), 1);
    }

    #[tokio::test]
    async fn extracts_from_commonjs_define_config_require() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("astro.config.cts");

        parse_config_document(
            r#"
                const { defineConfig: astroConfig } = require("astro/config");
                module.exports = astroConfig({
                    fonts: [{ cssVariable: "--font-commonjs-helper" }],
                });
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert_eq!(
            manager.get_variables("--font-commonjs-helper").await.len(),
            1
        );

        let direct_manager = CssVariableManager::new(Config::default());
        parse_config_document(
            r#"
                const defineConfig = require("astro/config").defineConfig;
                module.exports = defineConfig({
                    fonts: [{ cssVariable: "--font-commonjs-direct-helper" }],
                });
            "#,
            &uri,
            &direct_manager,
        )
        .await
        .unwrap();
        assert_eq!(
            direct_manager
                .get_variables("--font-commonjs-direct-helper")
                .await
                .len(),
            1
        );

        let namespace_manager = CssVariableManager::new(Config::default());
        parse_config_document(
            r#"
                const astro = require("astro/config");
                module.exports = astro.defineConfig({
                    fonts: [{ cssVariable: "--font-commonjs-namespace-helper" }],
                });
            "#,
            &uri,
            &namespace_manager,
        )
        .await
        .unwrap();
        assert_eq!(
            namespace_manager
                .get_variables("--font-commonjs-namespace-helper")
                .await
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn extracts_from_proven_namespace_define_config_imports_only() {
        let uri = test_uri("astro.config.mjs");
        let manager = CssVariableManager::new(Config::default());
        parse_config_document(
            r#"
                import * as astro from "astro/config";
                export default astro.defineConfig({
                    fonts: [{ cssVariable: "--font-esm-namespace-helper" }],
                });
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();
        assert_eq!(
            manager
                .get_variables("--font-esm-namespace-helper")
                .await
                .len(),
            1
        );

        let unproven_manager = CssVariableManager::new(Config::default());
        parse_config_document(
            r#"
                import * as astro from "unrelated-package";
                export default astro.defineConfig({
                    fonts: [{ cssVariable: "--font-unproven-namespace-helper" }],
                });
            "#,
            &uri,
            &unproven_manager,
        )
        .await
        .unwrap();
        assert!(unproven_manager
            .get_variables("--font-unproven-namespace-helper")
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn mutated_define_config_helpers_are_not_trusted() {
        let cjs_uri = test_uri("astro.config.cjs");
        for (name, source) in [
            (
                "--font-mutated-direct-helper",
                r#"
                    let defineConfig = require("astro/config").defineConfig;
                    defineConfig = runtimeHelper;
                    module.exports = defineConfig({
                        fonts: [{ cssVariable: "--font-mutated-direct-helper" }],
                    });
                "#,
            ),
            (
                "--font-mutated-namespace-helper",
                r#"
                    const astro = require("astro/config");
                    astro.defineConfig = runtimeHelper;
                    module.exports = astro.defineConfig({
                        fonts: [{ cssVariable: "--font-mutated-namespace-helper" }],
                    });
                "#,
            ),
        ] {
            let manager = CssVariableManager::new(Config::default());
            parse_config_document(source, &cjs_uri, &manager)
                .await
                .unwrap();
            assert!(manager.get_variables(name).await.is_empty(), "{name}");
        }

        let esm_manager = CssVariableManager::new(Config::default());
        let esm_uri = test_uri("astro.config.mjs");
        parse_config_document(
            r#"
                import * as astro from "astro/config";
                astro.defineConfig = runtimeHelper;
                export default astro.defineConfig({
                    fonts: [{ cssVariable: "--font-mutated-esm-namespace-helper" }],
                });
            "#,
            &esm_uri,
            &esm_manager,
        )
        .await
        .unwrap();
        assert!(esm_manager
            .get_variables("--font-mutated-esm-namespace-helper")
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn module_exports_is_ignored_in_explicit_esm_configs() {
        for name in ["astro.config.mjs", "astro.config.mts"] {
            let manager = CssVariableManager::new(Config::default());
            let uri = test_uri(name);
            parse_config_document(
                r#"module.exports = { fonts: [{ cssVariable: "--font-wrong-module" }] };"#,
                &uri,
                &manager,
            )
            .await
            .unwrap();
            assert!(manager
                .get_variables("--font-wrong-module")
                .await
                .is_empty());
        }
    }

    #[tokio::test]
    async fn malformed_and_escaped_values_are_not_indexed() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("astro.config.ts");

        parse_config_document(
            r#"export default { fonts: [{ cssVariable: "--font-escaped\x2dname" }] };"#,
            &uri,
            &manager,
        )
        .await
        .unwrap();
        assert!(manager
            .get_variables("--font-escaped-name")
            .await
            .is_empty());

        parse_config_document(
            r#"export default { fonts: [{ cssVariable: "--font-phantom" }]"#,
            &uri,
            &manager,
        )
        .await
        .unwrap();
        assert!(manager.get_variables("--font-phantom").await.is_empty());
    }

    #[tokio::test]
    async fn typescript_wrappers_and_unrelated_decorators_preserve_extraction() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("astro.config.ts");
        parse_config_document(
            r#"
                @sealed
                class ConfigMarker {}

                const CONFIG = {
                    fonts: [{ cssVariable: "--font-satisfies" }],
                } satisfies Record<string, unknown>;

                export default CONFIG;
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert_eq!(manager.get_variables("--font-satisfies").await.len(), 1);
    }

    #[tokio::test]
    async fn malformed_source_replaces_stale_state_with_safe_recovered_prefix() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("astro.config.ts");
        parse_config_document(
            r#"export default { fonts: [{ cssVariable: "--font-stale" }] };"#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        parse_config_document(
            r#"
                export default {
                    fonts: [{ cssVariable: "--font-safe-prefix" }],
                }; /*
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert!(manager.get_variables("--font-stale").await.is_empty());
        assert_eq!(manager.get_variables("--font-safe-prefix").await.len(), 1);
    }

    #[tokio::test]
    async fn oversized_config_preserves_the_last_valid_analysis() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("astro.config.ts");
        parse_config_document(
            r#"export default { fonts: [{ cssVariable: "--font-before-oversize" }] };"#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        let oversized = " ".repeat(MAX_CONFIG_BYTES + 1);
        parse_config_document(&oversized, &uri, &manager)
            .await
            .expect("oversized recognized configs should be skipped without an LSP error");

        assert_eq!(
            manager.get_variables("--font-before-oversize").await.len(),
            1
        );
    }

    #[tokio::test]
    async fn commonjs_exports_must_be_unconditional_top_level_assignments() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("astro.config.cjs");

        parse_config_document(
            r#"
                function unused() {
                    module.exports = { fonts: [{ cssVariable: "--font-nested" }] };
                }
                if (false) {
                    module.exports = { fonts: [{ cssVariable: "--font-conditional" }] };
                }
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert!(manager.get_variables("--font-nested").await.is_empty());
        assert!(manager.get_variables("--font-conditional").await.is_empty());
    }

    #[tokio::test]
    async fn commonjs_exports_reject_later_conditional_overrides() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("astro.config.cjs");

        parse_config_document(
            r#"
                module.exports = {
                    fonts: [{ cssVariable: "--font-before-conditional-override" }],
                };
                if (process.env.USE_OTHER_CONFIG) {
                    module.exports = { fonts: [] };
                }
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert!(manager
            .get_variables("--font-before-conditional-override")
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn define_config_helpers_mutated_inside_functions_are_rejected() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("vite.config.cjs");

        parse_config_document(
            r#"
                const vite = require("vite");
                (() => { vite.defineConfig = (value) => value; })();
                module.exports = vite.defineConfig({
                    css: {
                        preprocessorOptions: {
                            scss: {
                                additionalData: ":root { --vite-mutated-in-iife: red; }",
                            },
                        },
                    },
                });
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert!(manager
            .get_variables("--vite-mutated-in-iife")
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn function_parameter_shadowing_does_not_mutate_define_config_helpers() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("vite.config.cjs");

        parse_config_document(
            r#"
                const vite = require("vite");
                ((vite) => { vite.defineConfig = (value) => value; })({});
                module.exports = vite.defineConfig({
                    css: {
                        preprocessorOptions: {
                            scss: {
                                additionalData: ":root { --vite-outer-helper: red; }",
                            },
                        },
                    },
                });
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert_eq!(manager.get_variables("--vite-outer-helper").await.len(), 1);
    }

    #[tokio::test]
    async fn the_last_top_level_commonjs_export_wins() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("astro.config.cts");

        parse_config_document(
            r#"
                module.exports = { fonts: [{ cssVariable: "--font-old-export" }] };
                module.exports = { fonts: [{ cssVariable: "--font-current-export" }] };
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert!(manager.get_variables("--font-old-export").await.is_empty());
        assert_eq!(
            manager.get_variables("--font-current-export").await.len(),
            1
        );
    }

    #[tokio::test]
    async fn extraction_follows_effective_object_properties_conservatively() {
        let manager = CssVariableManager::new(Config::default());
        let uri = test_uri("astro.config.mjs");

        parse_config_document(
            r#"
                export default {
                    fonts: [{ cssVariable: "--font-overridden-root" }],
                    fonts: [
                        {
                            cssVariable: "--font-overridden-value",
                            cssVariable: "--font-effective",
                        },
                        {
                            cssVariable: "--font-possibly-overridden",
                            ...fontOverrides,
                        },
                        { cssVariable: "--font invalid" },
                    ],
                };
            "#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert_eq!(manager.get_variables("--font-effective").await.len(), 1);
        assert!(manager
            .get_variables("--font-overridden-root")
            .await
            .is_empty());
        assert!(manager
            .get_variables("--font-overridden-value")
            .await
            .is_empty());
        assert!(manager
            .get_variables("--font-possibly-overridden")
            .await
            .is_empty());
        assert!(manager.get_variables("--font invalid").await.is_empty());
    }
}
