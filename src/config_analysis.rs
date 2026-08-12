use std::collections::HashSet;
use std::path::Path;

use ls_types::{Range, Uri};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpression, AssignmentExpression, BindingPattern, ExportDefaultDeclaration,
    Expression, ImportDeclaration, ImportDeclarationSpecifier, ObjectExpression,
    ObjectPropertyKind, Program, PropertyKey, Statement, VariableDeclarator,
};
use oxc_ast_visit::Visit;
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

#[derive(Clone, Copy)]
enum ConfigKind {
    Astro,
    Vite,
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

/// Parse a recognized framework configuration file without executing it.
pub async fn parse_config_document(
    text: &str,
    uri: &Uri,
    manager: &CssVariableManager,
) -> Result<(), String> {
    if text.len() > MAX_CONFIG_BYTES {
        return Err(format!(
            "Configuration file exceeds the {} byte analysis limit",
            MAX_CONFIG_BYTES
        ));
    }

    let path = Path::new(uri.path().as_str());
    if config_kind(path).is_none() {
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
            value: "Astro generated font".to_string(),
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
    let kind = config_kind(path)
        .ok_or_else(|| format!("Unsupported configuration source: {}", path.display()))?;
    let source_type = SourceType::from_path(path)
        .map_err(|_| format!("Unsupported configuration source type: {}", path.display()))?;
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, source_type).parse();

    if !parsed.diagnostics.is_empty() {
        tracing::debug!(
            uri = ?uri,
            errors = parsed.diagnostics.len(),
            "configuration analysis skipped a malformed source"
        );
        return Ok(None);
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

    let has_esm_default = parsed
        .program
        .body
        .iter()
        .any(|statement| matches!(statement, Statement::ExportDefaultDeclaration(_)));
    let extracted = match kind {
        ConfigKind::Astro => {
            let mut extractor = AstroFontExtractor {
                source: text,
                define_config_bindings: imports.define_config_bindings,
                variables: Vec::new(),
            };
            extractor.visit_program(&parsed.program);
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
                define_config_bindings: imports.define_config_bindings,
                css_snippets: Vec::new(),
            };
            extractor.visit_program(&parsed.program);
            if supports_commonjs(path) && !has_esm_default {
                extractor.extract_commonjs_program(&parsed.program);
            }
            ConfigExtraction {
                variables: Vec::new(),
                css_snippets: extractor.css_snippets,
            }
        }
    };
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

struct DefineConfigImportCollector<'s> {
    module_source: &'s str,
    define_config_bindings: HashSet<String>,
}

impl<'s> DefineConfigImportCollector<'s> {
    fn new(module_source: &'s str) -> Self {
        Self {
            module_source,
            define_config_bindings: HashSet::new(),
        }
    }

    fn collect_commonjs_program(&mut self, program: &Program<'_>) {
        for statement in &program.body {
            let Statement::VariableDeclaration(declaration) = statement else {
                continue;
            };
            for declarator in &declaration.declarations {
                self.collect_commonjs_declarator(declarator);
            }
        }
    }

    fn collect_commonjs_declarator(&mut self, declarator: &VariableDeclarator<'_>) {
        let Some(Expression::CallExpression(call)) =
            declarator.init.as_ref().map(unwrap_expression)
        else {
            return;
        };
        if !call.is_require_call()
            || !matches!(
                call.arguments.first(),
                Some(Argument::StringLiteral(source))
                    if source.value.as_str() == self.module_source
            )
        {
            return;
        }

        let BindingPattern::ObjectPattern(pattern) = &declarator.id else {
            return;
        };
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
            let ImportDeclarationSpecifier::ImportSpecifier(specifier) = specifier else {
                continue;
            };
            if specifier.import_kind.is_value()
                && specifier.imported.name().as_str() == "defineConfig"
            {
                self.define_config_bindings
                    .insert(specifier.local.name.as_str().to_string());
            }
        }
    }
}

struct AstroFontExtractor<'s> {
    source: &'s str,
    define_config_bindings: HashSet<String>,
    variables: Vec<ExtractedVariable>,
}

impl AstroFontExtractor<'_> {
    fn extract_default_expression<'a>(&mut self, expression: &'a Expression<'a>) {
        if let Some(config) =
            config_object_from_expression(expression, &self.define_config_bindings)
        {
            self.extract_astro_config(config);
        }
    }

    fn extract_commonjs_program<'a>(&mut self, program: &'a Program<'a>) {
        if let Some(config) =
            commonjs_config_object(program, self.source, &self.define_config_bindings)
        {
            self.extract_astro_config(config);
        }
    }

    fn extract_astro_config<'a>(&mut self, config: &'a ObjectExpression<'a>) {
        if let Some(fonts) = object_property(config, "fonts").and_then(expression_as_array) {
            self.extract_fonts(fonts);
        }

        if let Some(experimental) = object_property(config, "experimental")
            .map(unwrap_expression)
            .and_then(as_object_expression)
        {
            if let Some(fonts) =
                object_property(experimental, "fonts").and_then(expression_as_array)
            {
                self.extract_fonts(fonts);
            }
        }
    }

    fn extract_fonts<'a>(&mut self, fonts: &'a ArrayExpression<'a>) {
        for element in &fonts.elements {
            let Some(expression) = element.as_expression() else {
                continue;
            };
            let Some(font) = as_object_expression(unwrap_expression(expression)) else {
                continue;
            };
            let Some(property) = object_property_node(font, "cssVariable") else {
                continue;
            };
            let Some((name, name_span)) = static_string_value(&property.value, self.source) else {
                continue;
            };
            if !is_supported_custom_property_name(&name) {
                continue;
            }

            self.variables.push(ExtractedVariable {
                name,
                declaration_span: property.span,
                name_span,
            });
        }
    }
}

impl<'a> Visit<'a> for AstroFontExtractor<'_> {
    fn visit_export_default_declaration(&mut self, declaration: &ExportDefaultDeclaration<'a>) {
        if let Some(expression) = declaration.declaration.as_expression() {
            self.extract_default_expression(expression);
        }
    }
}

struct ViteAdditionalDataExtractor<'s> {
    source: &'s str,
    define_config_bindings: HashSet<String>,
    css_snippets: Vec<ExtractedCssSnippet>,
}

impl ViteAdditionalDataExtractor<'_> {
    fn extract_default_expression<'a>(&mut self, expression: &'a Expression<'a>) {
        if let Some(config) =
            config_object_from_expression(expression, &self.define_config_bindings)
        {
            self.extract_vite_config(config);
        }
    }

    fn extract_commonjs_program<'a>(&mut self, program: &'a Program<'a>) {
        if let Some(config) =
            commonjs_config_object(program, self.source, &self.define_config_bindings)
        {
            self.extract_vite_config(config);
        }
    }

    fn extract_vite_config<'a>(&mut self, config: &'a ObjectExpression<'a>) {
        let Some(preprocessor_options) = object_property(config, "css")
            .map(unwrap_expression)
            .and_then(as_object_expression)
            .and_then(|css| object_property(css, "preprocessorOptions"))
            .map(unwrap_expression)
            .and_then(as_object_expression)
        else {
            return;
        };

        let Some(options) = object_property(preprocessor_options, VITE_PREPROCESSOR)
            .map(unwrap_expression)
            .and_then(as_object_expression)
        else {
            return;
        };
        let Some(additional_data) = object_property(options, "additionalData") else {
            return;
        };
        let Some((text, content_span)) = static_string_value(additional_data, self.source) else {
            return;
        };
        if contains_unsupported_scss_control_flow(&text) {
            return;
        }
        self.css_snippets
            .push(ExtractedCssSnippet { text, content_span });
    }
}

impl<'a> Visit<'a> for ViteAdditionalDataExtractor<'_> {
    fn visit_export_default_declaration(&mut self, declaration: &ExportDefaultDeclaration<'a>) {
        if let Some(expression) = declaration.declaration.as_expression() {
            self.extract_default_expression(expression);
        }
    }
}

fn config_object_from_expression<'a>(
    expression: &'a Expression<'a>,
    define_config_bindings: &HashSet<String>,
) -> Option<&'a ObjectExpression<'a>> {
    match unwrap_expression(expression) {
        Expression::ObjectExpression(object) => Some(object.as_ref()),
        Expression::CallExpression(call)
            if matches!(
                &call.callee,
                Expression::Identifier(identifier)
                    if define_config_bindings.contains(identifier.name.as_str())
            ) =>
        {
            call.arguments
                .first()
                .and_then(|argument| argument.as_expression())
                .map(unwrap_expression)
                .and_then(as_object_expression)
        }
        _ => None,
    }
}

fn commonjs_config_object<'a>(
    program: &'a Program<'a>,
    source: &str,
    define_config_bindings: &HashSet<String>,
) -> Option<&'a ObjectExpression<'a>> {
    for statement in program.body.iter().rev() {
        let Statement::ExpressionStatement(statement) = statement else {
            continue;
        };
        let Expression::AssignmentExpression(assignment) = unwrap_expression(&statement.expression)
        else {
            continue;
        };
        if is_module_exports_assignment(assignment, source) {
            return config_object_from_expression(&assignment.right, define_config_bindings);
        }
    }
    None
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

fn object_property_node<'a>(
    object: &'a ObjectExpression<'a>,
    name: &str,
) -> Option<&'a oxc_ast::ast::ObjectProperty<'a>> {
    for property in object.properties.iter().rev() {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            return None;
        };

        let matches = if property.computed {
            property.key.is_specific_string_literal(name)
        } else {
            property_key_matches(&property.key, name)
        };
        if matches {
            return Some(property.as_ref());
        }

        if property.computed && property.key.name().is_none() {
            return None;
        }
    }

    None
}

fn object_property<'a>(object: &'a ObjectExpression<'a>, name: &str) -> Option<&'a Expression<'a>> {
    object_property_node(object, name).map(|property| &property.value)
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

fn expression_as_array<'a>(expression: &'a Expression<'a>) -> Option<&'a ArrayExpression<'a>> {
    match unwrap_expression(expression) {
        Expression::ArrayExpression(array) => Some(array.as_ref()),
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

fn static_string_value(expression: &Expression<'_>, source: &str) -> Option<(String, Span)> {
    match unwrap_expression(expression) {
        Expression::StringLiteral(literal) => {
            let span = literal_content_span(literal.span);
            let raw = source.get(span.start as usize..span.end as usize)?;
            (raw == literal.value.as_str()).then_some((raw.to_string(), span))
        }
        Expression::TemplateLiteral(template)
            if template.expressions.is_empty() && template.quasis.len() == 1 =>
        {
            let span = literal_content_span(template.span);
            let raw = source.get(span.start as usize..span.end as usize)?;
            (!raw.contains('\\')).then_some((raw.to_string(), span))
        }
        _ => None,
    }
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

            const FONT_NAME = "--font-const";
            const FONT_ALIAS = FONT_NAME;

            export default defineConfig({
                fonts: [{ cssVariable: FONT_ALIAS }],
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
        assert_eq!(
            vite_manager
                .get_variables("--vite-const-derived")
                .await
                .len(),
            1
        );
        assert_eq!(vite_manager.get_usages("--base-color").await.len(), 1);
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
                const REASSIGNED = "--font-before-reassignment";
                REASSIGNED = "--font-after-reassignment";
                const DYNAMIC = `--font-${family}`;

                export default defineConfig({
                    fonts: [
                        { cssVariable: LET_NAME },
                        { cssVariable: VAR_NAME },
                        { cssVariable: REASSIGNED },
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
            "--font-before-reassignment",
            "--font-after-reassignment",
            "--font-after-use",
            "--font-local",
        ] {
            assert!(manager.get_variables(name).await.is_empty(), "{name}");
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
            const dynamicName = "--font-dynamic";
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

        parse_config_document(
            r#"export default { fonts: [{ cssVariable: "--font-direct" }] };"#,
            &uri,
            &manager,
        )
        .await
        .unwrap();

        assert_eq!(manager.get_variables("--font-direct").await.len(), 1);
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
