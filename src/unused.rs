use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, BindingPattern, CallExpression, Expression, ImportDeclaration,
    ImportDeclarationSpecifier, ModuleExportName, ObjectPropertyKind, VariableDeclarationKind,
    VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};

use crate::config::{ConfigMode, TransConfig};
use crate::error::{Result, TransError};
use crate::translations::{load_language_translations, save_language_translations};
use crate::verify::{
    TranslationSnapshot, restore_translations, snapshot_translations, verify_language_files,
};

const SOURCE_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mts", "cts"];
const FALLBACK_EXCLUDED_DIRS: &[&str] =
    &[".git", "node_modules", ".next", "dist", "build", "coverage"];
const TRANSLATOR_METHODS: &[&str] = &["rich", "markup", "raw", "has"];

#[derive(Debug, Clone, Default)]
pub struct UnusedReport {
    pub unused_ids: Vec<String>,
    pub warnings: Vec<String>,
    pub dynamic_usage_locations: Vec<UsageLocation>,
    pub dynamic_usage_detected: bool,
    pub extraction_usage_detected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageLocation {
    pub display: String,
    pub url: String,
}

#[derive(Debug, Clone, Default)]
pub struct UsageScan {
    pub used_ids: BTreeSet<String>,
    pub dynamic_usages: Vec<DynamicUsage>,
    pub extraction_usages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DynamicUsage {
    pub namespace: String,
    pub path: PathBuf,
    pub line: usize,
}

#[derive(Debug, Clone)]
struct TranslatorBinding {
    namespace: Option<String>,
    dynamic_namespace: bool,
}

#[derive(Debug, Clone)]
enum NamespaceArg {
    Scoped(String),
    Unscoped,
    Dynamic,
}

pub fn find_unused(root: impl AsRef<Path>, config: &TransConfig) -> Result<UnusedReport> {
    find_unused_with_options(root, config, true)
}

pub fn find_unused_keys(root: impl AsRef<Path>, config: &TransConfig) -> Result<Vec<String>> {
    Ok(find_unused_with_options(root, config, false)?.unused_ids)
}

pub fn remove_unused(
    root: impl AsRef<Path>,
    config: &TransConfig,
    force: bool,
) -> Result<UnusedReport> {
    let root = root.as_ref();
    let report = find_unused_with_options(root, config, !force)?;

    if report.extraction_usage_detected {
        return Err(TransError::InvalidInput(
            "cannot remove unused keys while next-intl extraction usage is detected".to_string(),
        ));
    }
    if report.dynamic_usage_detected && !force {
        return Err(TransError::InvalidInput(
            "dynamic translation key usage detected; rerun with --force to remove anyway"
                .to_string(),
        ));
    }
    if report.unused_ids.is_empty() {
        return Ok(report);
    }

    verify_language_files(root, config)?;
    let snapshot = snapshot_translations(root, config)?;
    let mut updated = snapshot.clone();
    let unused: BTreeSet<&str> = report.unused_ids.iter().map(String::as_str).collect();

    for language in &config.available_languages {
        let translations = updated.get_mut(language).ok_or_else(|| {
            TransError::InvalidInput(format!("missing translations for language '{language}'"))
        })?;
        translations.retain(|id, _| !unused.contains(id.as_str()));
    }

    if let Err(err) = write_snapshot(root, config, &updated) {
        let _ = restore_translations(root, config, &snapshot);
        return Err(err);
    }

    if let Err(err) = verify_language_files(root, config) {
        let _ = restore_translations(root, config, &snapshot);
        return Err(err);
    }

    Ok(report)
}

fn find_unused_with_options(
    root: impl AsRef<Path>,
    config: &TransConfig,
    apply_dynamic_exclusions: bool,
) -> Result<UnusedReport> {
    let root = root.as_ref();
    if config.mode != ConfigMode::NextIntl {
        return Err(TransError::InvalidInput(
            "trans unused currently supports next-intl mode only".to_string(),
        ));
    }

    verify_language_files(root, config)?;
    let primary = load_language_translations(root, config, &config.primary_language)?;
    let source_files = discover_source_files(root)?;
    let scan = collect_usage_from_files(&source_files)?;

    let mut unused_ids = Vec::new();
    for id in primary.keys() {
        if scan.used_ids.contains(id) {
            continue;
        }
        if apply_dynamic_exclusions && is_dynamic_protected(id, &scan.dynamic_usages) {
            continue;
        }
        unused_ids.push(id.clone());
    }

    let dynamic_usage_locations = dynamic_usage_locations(root, &scan.dynamic_usages);

    let mut warnings = Vec::new();
    if !scan.dynamic_usages.is_empty() {
        warnings.push(format!(
            "dynamic translation key usage detected in {} place(s)",
            dynamic_usage_locations.len()
        ));
    }
    if !scan.extraction_usages.is_empty() {
        warnings.push(format!(
            "next-intl extraction usage detected in {} place(s); remove is disabled for this project in v1",
            scan.extraction_usages.len()
        ));
    }

    Ok(UnusedReport {
        unused_ids,
        warnings,
        dynamic_usage_locations,
        dynamic_usage_detected: !scan.dynamic_usages.is_empty(),
        extraction_usage_detected: !scan.extraction_usages.is_empty(),
    })
}

fn write_snapshot(root: &Path, config: &TransConfig, snapshot: &TranslationSnapshot) -> Result<()> {
    for language in &config.available_languages {
        let translations = snapshot.get(language).ok_or_else(|| {
            TransError::InvalidInput(format!("missing translations for language '{language}'"))
        })?;
        save_language_translations(root, config, language, translations)?;
    }
    Ok(())
}

fn is_dynamic_protected(id: &str, dynamic_usages: &[DynamicUsage]) -> bool {
    dynamic_usages.iter().any(|prefix| {
        prefix.namespace.is_empty()
            || id == prefix.namespace
            || id
                .strip_prefix(&prefix.namespace)
                .is_some_and(|remaining| remaining.starts_with('.'))
    })
}

fn dynamic_usage_locations(root: &Path, dynamic_usages: &[DynamicUsage]) -> Vec<UsageLocation> {
    let mut locations: Vec<UsageLocation> = dynamic_usages
        .iter()
        .map(|usage| {
            let path = usage.path.strip_prefix(root).unwrap_or(&usage.path);
            UsageLocation {
                display: format!("./{}:{}", path.display(), usage.line),
                url: format!("file://{}#L{}", file_url_path(&usage.path), usage.line),
            }
        })
        .collect();
    locations.sort_by(|left, right| left.display.cmp(&right.display));
    locations.dedup_by(|left, right| left.display == right.display);
    locations
}

fn file_url_path(path: &Path) -> String {
    path.to_string_lossy()
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

fn collect_usage_from_files(paths: &[PathBuf]) -> Result<UsageScan> {
    let mut combined = UsageScan::default();
    for path in paths {
        let source = fs::read_to_string(path)?;
        let scan = collect_usage_from_source(&source, path)?;
        combined.used_ids.extend(scan.used_ids);
        combined.dynamic_usages.extend(scan.dynamic_usages);
        combined.extraction_usages.extend(scan.extraction_usages);
    }
    combined.dynamic_usages.sort();
    combined.dynamic_usages.dedup();
    combined.extraction_usages.sort();
    combined.extraction_usages.dedup();
    Ok(combined)
}

pub fn collect_usage_from_source(source: &str, path: &Path) -> Result<UsageScan> {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path).unwrap_or_default();
    let ret = Parser::new(&allocator, source, source_type).parse();
    if !ret.errors.is_empty() {
        return Err(TransError::InvalidInput(format!(
            "failed to parse source file '{}' ({} parser error(s))",
            path.display(),
            ret.errors.len()
        )));
    }

    let mut collector = SourceUsageCollector::new(path, source);
    collector.visit_program(&ret.program);
    Ok(collector.finish())
}

fn discover_source_files(root: &Path) -> Result<Vec<PathBuf>> {
    if let Some(files) = discover_source_files_with_git(root)? {
        return Ok(files);
    }

    let mut files = Vec::new();
    collect_source_files_fallback(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn discover_source_files_with_git(root: &Path) -> Result<Option<Vec<PathBuf>>> {
    let rev_parse = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--show-toplevel"])
        .output();
    let Ok(rev_parse) = rev_parse else {
        return Ok(None);
    };
    if !rev_parse.status.success() {
        return Ok(None);
    }

    let git_root = PathBuf::from(String::from_utf8_lossy(&rev_parse.stdout).trim());
    if git_root.as_os_str().is_empty() {
        return Ok(None);
    }

    let mut command = Command::new("git");
    command.arg("-C").arg(&git_root).args([
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
        "--",
    ]);

    let output = command.output()?;
    if !output.status.success() {
        return Ok(None);
    }

    let mut files = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let path = git_root.join(line);
        if path.starts_with(root) && is_source_file(&path) {
            files.push(path);
        }
    }
    files.sort();
    files.dedup();
    Ok(Some(files))
}

fn collect_source_files_fallback(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| FALLBACK_EXCLUDED_DIRS.contains(&name))
            {
                continue;
            }
            collect_source_files_fallback(&path, files)?;
        } else if file_type.is_file() && is_source_file(&path) {
            files.push(path);
        }
    }
    Ok(())
}

fn is_source_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| SOURCE_EXTENSIONS.contains(&extension))
}

#[derive(Debug)]
struct SourceUsageCollector {
    path: PathBuf,
    line_starts: Vec<usize>,
    use_translations: BTreeSet<String>,
    get_translations: BTreeSet<String>,
    use_extracted: BTreeSet<String>,
    get_extracted: BTreeSet<String>,
    string_constants: BTreeMap<String, String>,
    translators: BTreeMap<String, TranslatorBinding>,
    scan: UsageScan,
}

impl SourceUsageCollector {
    fn new(path: &Path, source: &str) -> Self {
        Self {
            path: path.to_path_buf(),
            line_starts: line_starts(source),
            use_translations: BTreeSet::new(),
            get_translations: BTreeSet::new(),
            use_extracted: BTreeSet::new(),
            get_extracted: BTreeSet::new(),
            string_constants: BTreeMap::new(),
            translators: BTreeMap::new(),
            scan: UsageScan::default(),
        }
    }

    fn finish(self) -> UsageScan {
        self.scan
    }

    fn record_import(&mut self, source: &str, imported: &str, local: &str) {
        match (source, imported) {
            ("next-intl", "useTranslations") => {
                self.use_translations.insert(local.to_string());
            }
            ("next-intl/server", "getTranslations") => {
                self.get_translations.insert(local.to_string());
            }
            ("next-intl", "useExtracted") => {
                self.use_extracted.insert(local.to_string());
            }
            ("next-intl/server", "getExtracted") => {
                self.get_extracted.insert(local.to_string());
            }
            _ => {}
        }
    }

    fn record_extraction_usage(&mut self) {
        self.scan
            .extraction_usages
            .push(self.path.display().to_string());
    }

    fn record_dynamic_usage(&mut self, namespace: Option<&str>, start: u32) {
        self.scan.dynamic_usages.push(DynamicUsage {
            namespace: namespace.unwrap_or_default().to_string(),
            path: self.path.clone(),
            line: self.line_number(start),
        });
    }

    fn line_number(&self, start: u32) -> usize {
        let offset = start as usize;
        self.line_starts
            .partition_point(|line_start| *line_start <= offset)
            .max(1)
    }

    fn call_namespace(&self, call: &CallExpression<'_>) -> NamespaceArg {
        let Some(first) = call.arguments.first() else {
            return NamespaceArg::Unscoped;
        };

        if let Some(value) = self.string_from_argument(first) {
            return NamespaceArg::Scoped(value);
        }

        match first {
            Argument::ObjectExpression(object) => {
                let mut spread = false;
                for property in &object.properties {
                    match property {
                        ObjectPropertyKind::ObjectProperty(property) => {
                            if property
                                .key
                                .static_name()
                                .is_some_and(|name| name == "namespace")
                            {
                                return self
                                    .string_from_expression(&property.value)
                                    .map(NamespaceArg::Scoped)
                                    .unwrap_or(NamespaceArg::Dynamic);
                            }
                        }
                        ObjectPropertyKind::SpreadProperty(_) => spread = true,
                    }
                }
                if spread {
                    NamespaceArg::Dynamic
                } else {
                    NamespaceArg::Unscoped
                }
            }
            _ => NamespaceArg::Dynamic,
        }
    }

    fn string_from_argument(&self, argument: &Argument<'_>) -> Option<String> {
        match argument {
            Argument::StringLiteral(literal) => Some(literal.value.to_string()),
            Argument::TemplateLiteral(literal) => {
                literal.single_quasi().map(|value| value.to_string())
            }
            Argument::Identifier(identifier) => {
                self.string_constants.get(identifier.name.as_str()).cloned()
            }
            _ => None,
        }
    }

    fn string_from_expression(&self, expression: &Expression<'_>) -> Option<String> {
        match expression.get_inner_expression() {
            Expression::StringLiteral(literal) => Some(literal.value.to_string()),
            Expression::TemplateLiteral(literal) => {
                literal.single_quasi().map(|value| value.to_string())
            }
            Expression::Identifier(identifier) => {
                self.string_constants.get(identifier.name.as_str()).cloned()
            }
            _ => None,
        }
    }

    fn callee_identifier<'a>(&self, expression: &'a Expression<'a>) -> Option<&'a str> {
        expression
            .get_inner_expression()
            .get_identifier_reference()
            .map(|identifier| identifier.name.as_str())
    }

    fn maybe_translator_call(&self, expression: &Expression<'_>) -> Option<TranslatorBinding> {
        if let Some(name) = self.callee_identifier(expression) {
            return self.translators.get(name).cloned();
        }

        let member = expression.get_member_expr()?;
        let method = member.static_property_name()?;
        if !TRANSLATOR_METHODS.contains(&method) {
            return None;
        }
        let object = member.object().get_identifier_reference()?;
        let name = object.name.as_str();
        self.translators.get(name).cloned()
    }

    fn maybe_translation_factory(&self, expression: &Expression<'_>) -> Option<bool> {
        let call = match expression.get_inner_expression() {
            Expression::CallExpression(call) => call,
            Expression::AwaitExpression(await_expression) => {
                match await_expression.argument.get_inner_expression() {
                    Expression::CallExpression(call) => call,
                    _ => return None,
                }
            }
            _ => return None,
        };
        let callee = self.callee_identifier(&call.callee)?;
        if self.use_translations.contains(callee) || self.get_translations.contains(callee) {
            Some(matches!(self.call_namespace(call), NamespaceArg::Dynamic))
        } else {
            None
        }
    }

    fn translator_binding_from_expression(
        &self,
        expression: &Expression<'_>,
    ) -> Option<TranslatorBinding> {
        let call = match expression.get_inner_expression() {
            Expression::CallExpression(call) => call,
            Expression::AwaitExpression(await_expression) => {
                match await_expression.argument.get_inner_expression() {
                    Expression::CallExpression(call) => call,
                    _ => return None,
                }
            }
            _ => return None,
        };
        let callee = self.callee_identifier(&call.callee)?;
        if !self.use_translations.contains(callee) && !self.get_translations.contains(callee) {
            return None;
        }

        Some(match self.call_namespace(call) {
            NamespaceArg::Scoped(namespace) => TranslatorBinding {
                namespace: Some(namespace),
                dynamic_namespace: false,
            },
            NamespaceArg::Unscoped => TranslatorBinding {
                namespace: None,
                dynamic_namespace: false,
            },
            NamespaceArg::Dynamic => TranslatorBinding {
                namespace: None,
                dynamic_namespace: true,
            },
        })
    }

    fn maybe_extraction_call(&self, call: &CallExpression<'_>) -> bool {
        let Some(callee) = self.callee_identifier(&call.callee) else {
            return false;
        };
        self.use_extracted.contains(callee) || self.get_extracted.contains(callee)
    }
}

impl<'a> Visit<'a> for SourceUsageCollector {
    fn visit_import_declaration(&mut self, declaration: &ImportDeclaration<'a>) {
        let source = declaration.source.value.as_str();
        if source == "next-intl" || source == "next-intl/server" {
            if let Some(specifiers) = &declaration.specifiers {
                for specifier in specifiers {
                    if let ImportDeclarationSpecifier::ImportSpecifier(specifier) = specifier {
                        let imported = match &specifier.imported {
                            ModuleExportName::IdentifierName(identifier) => {
                                identifier.name.as_str()
                            }
                            ModuleExportName::IdentifierReference(identifier) => {
                                identifier.name.as_str()
                            }
                            ModuleExportName::StringLiteral(literal) => literal.value.as_str(),
                        };
                        let local = specifier.local.name.as_str();
                        self.record_import(source, imported, local);
                    }
                }
            }
        }
        walk::walk_import_declaration(self, declaration);
    }

    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        if let BindingPattern::BindingIdentifier(identifier) = &declarator.id {
            let name = identifier.name.as_str();
            if declarator.kind == VariableDeclarationKind::Const {
                if let Some(init) = &declarator.init {
                    if let Some(value) = self.string_from_expression(init) {
                        self.string_constants.insert(name.to_string(), value);
                    }
                }
            }

            if let Some(init) = &declarator.init {
                if let Some(binding) = self.translator_binding_from_expression(init) {
                    self.translators.insert(name.to_string(), binding);
                } else if self.maybe_translation_factory(init).unwrap_or(false) {
                    self.record_dynamic_usage(None, init.span().start);
                }
            }
        }

        walk::walk_variable_declarator(self, declarator);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if self.maybe_extraction_call(call) {
            self.record_extraction_usage();
        }

        if let Some(binding) = self.maybe_translator_call(&call.callee) {
            let key = call
                .arguments
                .first()
                .and_then(|argument| self.string_from_argument(argument));
            match key {
                Some(key) if !binding.dynamic_namespace => {
                    let id = match &binding.namespace {
                        Some(namespace) if key.is_empty() => namespace.clone(),
                        Some(namespace) => format!("{namespace}.{key}"),
                        None => key,
                    };
                    self.scan.used_ids.insert(id);
                }
                _ => self.record_dynamic_usage(binding.namespace.as_deref(), call.span.start),
            }
        }

        walk::walk_call_expression(self, call);
    }
}

fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(source: &str) -> UsageScan {
        collect_usage_from_source(source, Path::new("sample.tsx")).expect("scan")
    }

    #[test]
    fn collects_scoped_use_translations() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations('common');
            t('save');
            "#,
        );
        assert!(scan.used_ids.contains("common.save"));
    }

    #[test]
    fn collects_unscoped_use_translations() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations();
            t('common.save');
            "#,
        );
        assert!(scan.used_ids.contains("common.save"));
    }

    #[test]
    fn collects_get_translations_namespaces() {
        let scan = scan(
            r#"
            import {getTranslations} from 'next-intl/server';
            const a = await getTranslations('auth');
            const b = await getTranslations({locale, namespace: 'metadata'});
            a('login');
            b('title');
            "#,
        );
        assert!(scan.used_ids.contains("auth.login"));
        assert!(scan.used_ids.contains("metadata.title"));
    }

    #[test]
    fn collects_method_variants() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations('common');
            t.rich('body');
            t.markup('html');
            t.raw('payload');
            t.has('optional');
            "#,
        );
        assert!(scan.used_ids.contains("common.body"));
        assert!(scan.used_ids.contains("common.html"));
        assert!(scan.used_ids.contains("common.payload"));
        assert!(scan.used_ids.contains("common.optional"));
    }

    #[test]
    fn resolves_aliases_and_string_constants() {
        let scan = scan(
            r#"
            import {useTranslations as useT} from 'next-intl';
            const namespace = 'common';
            const key = 'save';
            const t = useT(namespace);
            t(key);
            "#,
        );
        assert!(scan.used_ids.contains("common.save"));
    }

    #[test]
    fn detects_dynamic_keys_and_namespaces() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations(section);
            t('save');
            const common = useTranslations('common');
            common(key);
            "#,
        );
        assert!(
            scan.dynamic_usages
                .iter()
                .any(|usage| usage.namespace.is_empty())
        );
        assert!(
            scan.dynamic_usages
                .iter()
                .any(|usage| usage.namespace == "common")
        );
    }

    #[test]
    fn detects_extraction_usage() {
        let scan = scan(
            r#"
            import {useExtracted} from 'next-intl';
            import {getExtracted as getX} from 'next-intl/server';
            const t = useExtracted();
            const s = await getX();
            "#,
        );
        assert_eq!(scan.extraction_usages.len(), 2);
    }
}
