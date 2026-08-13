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

fn supports_commonjs(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("js" | "cjs" | "ts" | "cts")
    )
}

/// Return whether a path is a supported framework configuration source.
pub fn is_supported_config_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    ASTRO_CONFIG_NAMES.contains(&name)
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
    if !is_supported_config_path(path) {
        return Ok(());
    }

    let Some(extracted_variables) = extract_config_variables(text, path, uri)? else {
        return Ok(());
    };
    let variables = extracted_variables
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
    manager.replace_document_variables(uri, variables).await
}

fn extract_config_variables(
    text: &str,
    path: &Path,
    uri: &Uri,
) -> Result<Option<Vec<ExtractedVariable>>, String> {
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

    let mut imports = AstroImportCollector::default();
    imports.visit_program(&parsed.program);
    if supports_commonjs(path) {
        imports.collect_commonjs_program(&parsed.program);
    }

    let mut extractor = AstroFontExtractor {
        source: text,
        define_config_bindings: imports.define_config_bindings,
        variables: Vec::new(),
    };
    extractor.visit_program(&parsed.program);
    if supports_commonjs(path) {
        extractor.extract_commonjs_program(&parsed.program);
    }
    Ok(Some(extractor.variables))
}

struct ExtractedVariable {
    name: String,
    declaration_span: Span,
    name_span: Span,
}

#[derive(Default)]
struct AstroImportCollector {
    define_config_bindings: HashSet<String>,
}

impl AstroImportCollector {
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
                Some(Argument::StringLiteral(source)) if source.value.as_str() == "astro/config"
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

impl<'a> Visit<'a> for AstroImportCollector {
    fn visit_import_declaration(&mut self, declaration: &ImportDeclaration<'a>) {
        if declaration.source.value.as_str() != "astro/config" {
            return;
        }
        let Some(specifiers) = declaration.specifiers.as_ref() else {
            return;
        };

        for specifier in specifiers {
            let ImportDeclarationSpecifier::ImportSpecifier(specifier) = specifier else {
                continue;
            };
            if specifier.imported.name().as_str() == "defineConfig" {
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
    fn is_define_config_call(&self, expression: &Expression<'_>) -> bool {
        matches!(
            expression,
            Expression::Identifier(identifier)
                if self.define_config_bindings.contains(identifier.name.as_str())
        )
    }

    fn extract_default_expression<'a>(&mut self, expression: &'a Expression<'a>) {
        let expression = unwrap_expression(expression);
        let config = match expression {
            Expression::ObjectExpression(object) => Some(object.as_ref()),
            Expression::CallExpression(call) if self.is_define_config_call(&call.callee) => call
                .arguments
                .first()
                .and_then(|argument| argument.as_expression())
                .map(unwrap_expression)
                .and_then(as_object_expression),
            _ => None,
        };

        if let Some(config) = config {
            self.extract_astro_config(config);
        }
    }

    fn extract_commonjs_program<'a>(&mut self, program: &'a Program<'a>) {
        for statement in program.body.iter().rev() {
            let Statement::ExpressionStatement(statement) = statement else {
                continue;
            };
            let Expression::AssignmentExpression(assignment) =
                unwrap_expression(&statement.expression)
            else {
                continue;
            };
            if is_module_exports_assignment(assignment, self.source) {
                self.extract_default_expression(&assignment.right);
                break;
            }
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
            let quasi = &template.quasis[0];
            let value = quasi
                .value
                .cooked
                .as_ref()
                .unwrap_or(&quasi.value.raw)
                .as_str()
                .to_string();
            let span = literal_content_span(template.span);
            let raw = source.get(span.start as usize..span.end as usize)?;
            (raw == value).then_some((value, span))
        }
        _ => None,
    }
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
    fn recognizes_only_supported_astro_config_names() {
        assert!(is_supported_config_path(Path::new("astro.config.mjs")));
        assert!(is_supported_config_path(Path::new(
            "/workspace/astro.config.ts"
        )));
        assert!(!is_supported_config_path(Path::new("astro.config.json")));
        assert!(!is_supported_config_path(Path::new("astro.config.foo.ts")));
        assert!(!is_supported_config_path(Path::new("src/config.ts")));
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
