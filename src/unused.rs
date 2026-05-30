use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, ArrowFunctionExpression, BindingPattern, CallExpression,
    ConditionalExpression, Declaration, ExportDefaultDeclaration, ExportDefaultDeclarationKind,
    ExportNamedDeclaration, Expression, ForOfStatement, ForStatementLeft, Function, FunctionBody,
    ImportDeclaration, ImportDeclarationSpecifier, ModuleExportName, ObjectPropertyKind,
    ReturnStatement, Statement, TemplateLiteral, VariableDeclaration, VariableDeclarationKind,
    VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};
use oxc_syntax::scope::ScopeFlags;
use serde::Deserialize;

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
const MAX_FINITE_STRINGS: usize = 128;

type FiniteStrings = BTreeSet<String>;

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

#[derive(Debug, Clone, Default)]
struct ProjectIndex {
    files: BTreeMap<PathBuf, SourceFileIndex>,
    source_files: BTreeSet<PathBuf>,
    aliases: PathAliases,
}

#[derive(Debug, Clone, Default)]
struct SourceFileIndex {
    imports: BTreeMap<String, ImportTarget>,
    helpers: BTreeMap<String, HelperSummary>,
    return_helpers: BTreeMap<String, FiniteStrings>,
    named_exports: BTreeMap<String, HelperSummary>,
    named_return_exports: BTreeMap<String, FiniteStrings>,
    default_export: Option<HelperSummary>,
    default_return_export: Option<FiniteStrings>,
}

#[derive(Debug, Clone)]
struct ImportTarget {
    source: String,
    imported: ImportedName,
}

#[derive(Debug, Clone)]
enum ImportedName {
    Named(String),
    Default,
}

#[derive(Debug, Clone, Default)]
struct HelperSummary {
    param_usages: Vec<HelperParamUsage>,
}

#[derive(Debug, Clone)]
struct HelperParamUsage {
    param_index: usize,
    keys: Option<FiniteStrings>,
}

#[derive(Debug, Clone, Default)]
struct PathAliases {
    base_url: Option<PathBuf>,
    paths: Vec<PathAlias>,
}

#[derive(Debug, Clone)]
struct PathAlias {
    prefix: String,
    suffix: String,
    targets: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TsConfig {
    compiler_options: Option<TsCompilerOptions>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TsCompilerOptions {
    base_url: Option<String>,
    paths: Option<BTreeMap<String, Vec<String>>>,
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
    let scan = collect_usage_from_files(root, &source_files)?;

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

fn collect_usage_from_files(root: &Path, paths: &[PathBuf]) -> Result<UsageScan> {
    let project = ProjectIndex::build(root, paths)?;
    let mut combined = UsageScan::default();
    for path in paths {
        let source = fs::read_to_string(path)?;
        let scan = collect_usage_from_source_with_project(&source, path, Some(&project))?;
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
    collect_usage_from_source_with_project(source, path, None)
}

fn collect_usage_from_source_with_project(
    source: &str,
    path: &Path,
    project: Option<&ProjectIndex>,
) -> Result<UsageScan> {
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

    let local_project;
    let project = match project {
        Some(project) => Some(project),
        None => {
            let mut index_collector = SourceIndexCollector::default();
            index_collector.visit_program(&ret.program);
            let mut files = BTreeMap::new();
            files.insert(path.to_path_buf(), index_collector.finish());
            let mut source_files = BTreeSet::new();
            source_files.insert(path.to_path_buf());
            local_project = ProjectIndex {
                files,
                source_files,
                aliases: PathAliases::default(),
            };
            Some(&local_project)
        }
    };

    let mut collector = SourceUsageCollector::new(path, source, project);
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

impl ProjectIndex {
    fn build(root: &Path, paths: &[PathBuf]) -> Result<Self> {
        let aliases = PathAliases::load(root);
        let source_files: BTreeSet<PathBuf> = paths.iter().cloned().collect();
        let mut files = BTreeMap::new();

        for path in paths {
            let source = fs::read_to_string(path)?;
            let allocator = Allocator::default();
            let source_type = SourceType::from_path(path).unwrap_or_default();
            let ret = Parser::new(&allocator, &source, source_type).parse();
            if !ret.errors.is_empty() {
                return Err(TransError::InvalidInput(format!(
                    "failed to parse source file '{}' ({} parser error(s))",
                    path.display(),
                    ret.errors.len()
                )));
            }

            let mut collector = SourceIndexCollector::default();
            collector.visit_program(&ret.program);
            files.insert(path.clone(), collector.finish());
        }

        Ok(Self {
            files,
            source_files,
            aliases,
        })
    }

    fn helper_for_import(&self, from: &Path, target: &ImportTarget) -> Option<HelperSummary> {
        let path = self.resolve_module(from, &target.source)?;
        let file = self.files.get(&path)?;
        match &target.imported {
            ImportedName::Named(name) => file.named_exports.get(name).cloned(),
            ImportedName::Default => file.default_export.clone(),
        }
    }

    fn return_helper_for_import(
        &self,
        from: &Path,
        target: &ImportTarget,
    ) -> Option<FiniteStrings> {
        let path = self.resolve_module(from, &target.source)?;
        let file = self.files.get(&path)?;
        match &target.imported {
            ImportedName::Named(name) => file.named_return_exports.get(name).cloned(),
            ImportedName::Default => file.default_return_export.clone(),
        }
    }

    fn resolve_module(&self, from: &Path, source: &str) -> Option<PathBuf> {
        if source.starts_with('.') {
            let base = from.parent()?.join(source);
            return self.resolve_candidate(&base);
        }

        for candidate in self.aliases.expand(source) {
            if let Some(path) = self.resolve_candidate(&candidate) {
                return Some(path);
            }
        }

        None
    }

    fn resolve_candidate(&self, base: &Path) -> Option<PathBuf> {
        let candidates = module_candidates(base);
        candidates
            .into_iter()
            .map(|candidate| normalize_path(&candidate))
            .find(|candidate| self.source_files.contains(candidate))
    }
}

impl SourceFileIndex {
    fn helper_for_local(&self, name: &str) -> Option<HelperSummary> {
        self.helpers.get(name).cloned()
    }

    fn return_helper_for_local(&self, name: &str) -> Option<FiniteStrings> {
        self.return_helpers.get(name).cloned()
    }
}

impl PathAliases {
    fn load(root: &Path) -> Self {
        for file_name in ["tsconfig.json", "jsconfig.json"] {
            let path = root.join(file_name);
            let Ok(contents) = fs::read_to_string(&path) else {
                continue;
            };
            let stripped = strip_json_comments(&contents);
            let Ok(config) = serde_json::from_str::<TsConfig>(&stripped) else {
                continue;
            };
            let Some(options) = config.compiler_options else {
                return Self::default();
            };

            let base_url = options.base_url.map(|value| root.join(value));
            let base = base_url.clone().unwrap_or_else(|| root.to_path_buf());
            let mut paths = Vec::new();
            if let Some(config_paths) = options.paths {
                for (alias, targets) in config_paths {
                    let (prefix, suffix) = split_alias_pattern(&alias);
                    paths.push(PathAlias {
                        prefix,
                        suffix,
                        targets: targets
                            .into_iter()
                            .map(|target| base.join(target).to_string_lossy().to_string())
                            .collect(),
                    });
                }
            }

            return Self { base_url, paths };
        }

        Self::default()
    }

    fn expand(&self, source: &str) -> Vec<PathBuf> {
        let mut candidates = Vec::new();
        for alias in &self.paths {
            if let Some(capture) = match_alias(source, &alias.prefix, &alias.suffix) {
                for target in &alias.targets {
                    let path = if target.contains('*') {
                        target.replace('*', capture)
                    } else {
                        target.clone()
                    };
                    candidates.push(PathBuf::from(path));
                }
            }
        }

        if candidates.is_empty() {
            if let Some(base_url) = &self.base_url {
                candidates.push(base_url.join(source));
            }
        }

        candidates
    }
}

fn module_candidates(base: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if is_source_file(base) {
        candidates.push(base.to_path_buf());
    } else {
        for extension in SOURCE_EXTENSIONS {
            candidates.push(path_with_appended_extension(base, extension));
        }
    }

    for extension in SOURCE_EXTENSIONS {
        candidates.push(base.join(format!("index.{extension}")));
    }

    candidates
}

fn path_with_appended_extension(base: &Path, extension: &str) -> PathBuf {
    let mut path = base.as_os_str().to_os_string();
    path.push(".");
    path.push(extension);
    PathBuf::from(path)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn split_alias_pattern(pattern: &str) -> (String, String) {
    match pattern.split_once('*') {
        Some((prefix, suffix)) => (prefix.to_string(), suffix.to_string()),
        None => (pattern.to_string(), String::new()),
    }
}

fn match_alias<'a>(source: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    if suffix.is_empty() && source == prefix {
        return Some("");
    }
    source.strip_prefix(prefix)?.strip_suffix(suffix)
}

fn strip_json_comments(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if in_string {
            escaped = ch == '\\' && !escaped;
            if ch == '"' && !escaped {
                in_string = false;
            }
            output.push(ch);
            if ch != '\\' {
                escaped = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            output.push(ch);
            continue;
        }

        if ch == '/' {
            match chars.peek().copied() {
                Some('/') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if next == '\n' {
                            output.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
                    chars.next();
                    let mut previous = '\0';
                    for next in chars.by_ref() {
                        if previous == '*' && next == '/' {
                            break;
                        }
                        previous = next;
                    }
                    continue;
                }
                _ => {}
            }
        }

        output.push(ch);
    }

    output
}

#[derive(Debug)]
struct SourceUsageCollector {
    path: PathBuf,
    line_starts: Vec<usize>,
    project: Option<ProjectIndex>,
    file_index: Option<SourceFileIndex>,
    use_translations: BTreeSet<String>,
    get_translations: BTreeSet<String>,
    use_extracted: BTreeSet<String>,
    get_extracted: BTreeSet<String>,
    finite_constants: BTreeMap<String, FiniteStrings>,
    finite_iterables: BTreeMap<String, FiniteStrings>,
    translators: BTreeMap<String, TranslatorBinding>,
    scan: UsageScan,
}

fn helper_summary_from_expression(expression: &Expression<'_>) -> Option<HelperSummary> {
    match expression.get_inner_expression() {
        Expression::ArrowFunctionExpression(arrow) => Some(helper_summary_from_arrow(arrow)),
        Expression::FunctionExpression(function) => helper_summary_from_function(function),
        _ => None,
    }
}

fn helper_summary_from_arrow(arrow: &ArrowFunctionExpression<'_>) -> HelperSummary {
    helper_summary_from_body(&arrow.params, &arrow.body)
}

fn helper_summary_from_function(function: &Function<'_>) -> Option<HelperSummary> {
    let body = function.body.as_ref()?;
    Some(helper_summary_from_body(&function.params, body))
}

fn helper_summary_from_body(
    params: &oxc_ast::ast::FormalParameters<'_>,
    body: &FunctionBody<'_>,
) -> HelperSummary {
    let mut param_names = BTreeMap::new();
    for (index, parameter) in params.items.iter().enumerate() {
        if let Some(name) = binding_identifier_name(&parameter.pattern) {
            param_names.insert(name.to_string(), index);
        }
    }

    let mut collector = HelperBodyCollector {
        param_names,
        finite_constants: BTreeMap::new(),
        finite_iterables: BTreeMap::new(),
        usages: Vec::new(),
    };
    for statement in &body.statements {
        collector.visit_statement(statement);
    }

    HelperSummary {
        param_usages: collector.usages,
    }
}

fn finite_return_summary_from_expression(expression: &Expression<'_>) -> Option<FiniteStrings> {
    match expression.get_inner_expression() {
        Expression::ArrowFunctionExpression(arrow) => finite_return_summary_from_arrow(arrow),
        Expression::FunctionExpression(function) => finite_return_summary_from_function(function),
        _ => None,
    }
}

fn finite_return_summary_from_arrow(arrow: &ArrowFunctionExpression<'_>) -> Option<FiniteStrings> {
    if arrow.expression {
        if let Some(Statement::ExpressionStatement(statement)) = arrow.body.statements.first() {
            return finite_strings_from_expression(&statement.expression, &BTreeMap::new());
        }
    }
    finite_return_summary_from_body(&arrow.body)
}

fn finite_return_summary_from_function(function: &Function<'_>) -> Option<FiniteStrings> {
    let body = function.body.as_ref()?;
    finite_return_summary_from_body(body)
}

fn finite_return_summary_from_body(body: &FunctionBody<'_>) -> Option<FiniteStrings> {
    let mut collector = ReturnValueCollector {
        finite_constants: BTreeMap::new(),
        finite_iterables: BTreeMap::new(),
        return_values: BTreeSet::new(),
        unknown_return: false,
    };
    for statement in &body.statements {
        collector.visit_statement(statement);
    }
    if collector.unknown_return || collector.return_values.is_empty() {
        None
    } else {
        Some(collector.return_values)
    }
}

fn binding_identifier_name<'a>(pattern: &'a BindingPattern<'a>) -> Option<&'a str> {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => Some(identifier.name.as_str()),
        BindingPattern::AssignmentPattern(assignment) => binding_identifier_name(&assignment.left),
        _ => None,
    }
}

fn module_export_name<'a>(name: &'a ModuleExportName<'a>) -> Option<&'a str> {
    match name {
        ModuleExportName::IdentifierName(identifier) => Some(identifier.name.as_str()),
        ModuleExportName::IdentifierReference(identifier) => Some(identifier.name.as_str()),
        ModuleExportName::StringLiteral(literal) => Some(literal.value.as_str()),
    }
}

fn finite_string(value: impl Into<String>) -> FiniteStrings {
    BTreeSet::from([value.into()])
}

fn single_finite_string(values: &FiniteStrings) -> Option<String> {
    if values.len() == 1 {
        values.first().cloned()
    } else {
        None
    }
}

fn union_finite_strings(left: FiniteStrings, right: FiniteStrings) -> Option<FiniteStrings> {
    let mut values = left;
    values.extend(right);
    if values.len() > MAX_FINITE_STRINGS {
        None
    } else {
        Some(values)
    }
}

fn append_finite_strings(
    prefixes: FiniteStrings,
    suffixes: &FiniteStrings,
) -> Option<FiniteStrings> {
    let mut values = BTreeSet::new();
    for prefix in prefixes {
        for suffix in suffixes {
            values.insert(format!("{prefix}{suffix}"));
            if values.len() > MAX_FINITE_STRINGS {
                return None;
            }
        }
    }
    Some(values)
}

fn finite_strings_from_argument(
    argument: &Argument<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
) -> Option<FiniteStrings> {
    match argument {
        Argument::StringLiteral(literal) => Some(finite_string(literal.value.to_string())),
        Argument::TemplateLiteral(literal) => finite_strings_from_template(literal, constants),
        Argument::Identifier(identifier) => constants.get(identifier.name.as_str()).cloned(),
        Argument::CallExpression(call) => finite_strings_from_call(call, constants),
        Argument::ConditionalExpression(conditional) => {
            finite_strings_from_conditional(conditional, constants)
        }
        Argument::ParenthesizedExpression(parenthesized) => {
            finite_strings_from_expression(&parenthesized.expression, constants)
        }
        Argument::TSAsExpression(expression) => {
            finite_strings_from_expression(&expression.expression, constants)
        }
        Argument::TSSatisfiesExpression(expression) => {
            finite_strings_from_expression(&expression.expression, constants)
        }
        Argument::TSNonNullExpression(expression) => {
            finite_strings_from_expression(&expression.expression, constants)
        }
        Argument::TSInstantiationExpression(expression) => {
            finite_strings_from_expression(&expression.expression, constants)
        }
        Argument::TSTypeAssertion(expression) => {
            finite_strings_from_expression(&expression.expression, constants)
        }
        _ => None,
    }
}

fn finite_strings_from_expression(
    expression: &Expression<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
) -> Option<FiniteStrings> {
    match expression.get_inner_expression() {
        Expression::StringLiteral(literal) => Some(finite_string(literal.value.to_string())),
        Expression::TemplateLiteral(literal) => finite_strings_from_template(literal, constants),
        Expression::Identifier(identifier) => constants.get(identifier.name.as_str()).cloned(),
        Expression::CallExpression(call) => finite_strings_from_call(call, constants),
        Expression::ConditionalExpression(conditional) => {
            finite_strings_from_conditional(conditional, constants)
        }
        _ => None,
    }
}

fn finite_iterable_from_expression(
    expression: &Expression<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    iterables: &BTreeMap<String, FiniteStrings>,
) -> Option<FiniteStrings> {
    match expression.get_inner_expression() {
        Expression::ArrayExpression(array) => {
            finite_iterable_from_array_elements(array.elements.iter(), constants, iterables)
        }
        Expression::Identifier(identifier) => iterables.get(identifier.name.as_str()).cloned(),
        _ => None,
    }
}

fn finite_iterable_from_array_elements<'a>(
    elements: impl Iterator<Item = &'a ArrayExpressionElement<'a>>,
    constants: &BTreeMap<String, FiniteStrings>,
    iterables: &BTreeMap<String, FiniteStrings>,
) -> Option<FiniteStrings> {
    let mut values = BTreeSet::new();
    for element in elements {
        let element_values = finite_strings_from_array_element(element, constants, iterables)?;
        values.extend(element_values);
        if values.len() > MAX_FINITE_STRINGS {
            return None;
        }
    }
    Some(values)
}

fn finite_strings_from_array_element(
    element: &ArrayExpressionElement<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    iterables: &BTreeMap<String, FiniteStrings>,
) -> Option<FiniteStrings> {
    match element {
        ArrayExpressionElement::StringLiteral(literal) => {
            Some(finite_string(literal.value.to_string()))
        }
        ArrayExpressionElement::TemplateLiteral(literal) => {
            finite_strings_from_template(literal, constants)
        }
        ArrayExpressionElement::Identifier(identifier) => {
            constants.get(identifier.name.as_str()).cloned()
        }
        ArrayExpressionElement::CallExpression(call) => finite_strings_from_call(call, constants),
        ArrayExpressionElement::ConditionalExpression(conditional) => {
            finite_strings_from_conditional(conditional, constants)
        }
        ArrayExpressionElement::ParenthesizedExpression(parenthesized) => {
            finite_strings_from_expression(&parenthesized.expression, constants)
        }
        ArrayExpressionElement::TSAsExpression(expression) => {
            finite_iterable_from_expression(&expression.expression, constants, iterables)
                .or_else(|| finite_strings_from_expression(&expression.expression, constants))
        }
        ArrayExpressionElement::TSSatisfiesExpression(expression) => {
            finite_iterable_from_expression(&expression.expression, constants, iterables)
                .or_else(|| finite_strings_from_expression(&expression.expression, constants))
        }
        ArrayExpressionElement::TSNonNullExpression(expression) => {
            finite_iterable_from_expression(&expression.expression, constants, iterables)
                .or_else(|| finite_strings_from_expression(&expression.expression, constants))
        }
        ArrayExpressionElement::TSInstantiationExpression(expression) => {
            finite_iterable_from_expression(&expression.expression, constants, iterables)
                .or_else(|| finite_strings_from_expression(&expression.expression, constants))
        }
        ArrayExpressionElement::TSTypeAssertion(expression) => {
            finite_iterable_from_expression(&expression.expression, constants, iterables)
                .or_else(|| finite_strings_from_expression(&expression.expression, constants))
        }
        _ => None,
    }
}

fn finite_strings_from_call(
    call: &CallExpression<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
) -> Option<FiniteStrings> {
    if !call.arguments.is_empty() {
        return None;
    }

    let member = call.callee.get_member_expr()?;
    let method = member.static_property_name()?;
    let values = finite_strings_from_expression(member.object(), constants)?;

    transform_finite_strings(values, &method)
}

fn transform_finite_strings(values: FiniteStrings, method: &str) -> Option<FiniteStrings> {
    let transformed = match method {
        "toLowerCase" => values
            .into_iter()
            .map(|value| value.to_lowercase())
            .collect(),
        "toUpperCase" => values
            .into_iter()
            .map(|value| value.to_uppercase())
            .collect(),
        _ => return None,
    };
    Some(transformed)
}

fn finite_strings_from_conditional(
    conditional: &ConditionalExpression<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
) -> Option<FiniteStrings> {
    let consequent = finite_strings_from_expression(&conditional.consequent, constants)?;
    let alternate = finite_strings_from_expression(&conditional.alternate, constants)?;
    union_finite_strings(consequent, alternate)
}

fn finite_strings_from_template(
    literal: &TemplateLiteral<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
) -> Option<FiniteStrings> {
    let mut values = finite_string("");
    for (index, quasi) in literal.quasis.iter().enumerate() {
        let cooked = quasi.value.cooked.as_ref()?;
        values = append_finite_strings(values, &finite_string(cooked.to_string()))?;

        if let Some(expression) = literal.expressions.get(index) {
            let expression_values = finite_strings_from_expression(expression, constants)?;
            values = append_finite_strings(values, &expression_values)?;
        }
    }
    Some(values)
}

struct HelperBodyCollector {
    param_names: BTreeMap<String, usize>,
    finite_constants: BTreeMap<String, FiniteStrings>,
    finite_iterables: BTreeMap<String, FiniteStrings>,
    usages: Vec<HelperParamUsage>,
}

impl HelperBodyCollector {
    fn callee_param_index(&self, expression: &Expression<'_>) -> Option<usize> {
        if let Some(identifier) = expression.get_identifier_reference() {
            return self.param_names.get(identifier.name.as_str()).copied();
        }

        let member = expression.get_member_expr()?;
        let method = member.static_property_name()?;
        if !TRANSLATOR_METHODS.contains(&method) {
            return None;
        }
        let object = member.object().get_identifier_reference()?;
        self.param_names.get(object.name.as_str()).copied()
    }

    fn finite_iterable_from_expression(
        &self,
        expression: &Expression<'_>,
    ) -> Option<FiniteStrings> {
        finite_iterable_from_expression(expression, &self.finite_constants, &self.finite_iterables)
    }

    fn with_finite_constant(
        &mut self,
        name: &str,
        values: FiniteStrings,
        visit: impl FnOnce(&mut Self),
    ) {
        let previous = self.finite_constants.insert(name.to_string(), values);
        visit(self);
        match previous {
            Some(values) => {
                self.finite_constants.insert(name.to_string(), values);
            }
            None => {
                self.finite_constants.remove(name);
            }
        }
    }

    fn visit_finite_iteration_callback(
        &mut self,
        callback: &Argument<'_>,
        values: FiniteStrings,
    ) -> bool {
        match callback {
            Argument::ArrowFunctionExpression(arrow) => {
                let Some(param) = arrow
                    .params
                    .items
                    .first()
                    .and_then(|param| binding_identifier_name(&param.pattern))
                else {
                    return false;
                };
                self.with_finite_constant(param, values, |collector| {
                    walk::walk_arrow_function_expression(collector, arrow);
                });
                true
            }
            Argument::FunctionExpression(function) => {
                let Some(param) = function
                    .params
                    .items
                    .first()
                    .and_then(|param| binding_identifier_name(&param.pattern))
                else {
                    return false;
                };
                self.with_finite_constant(param, values, |collector| {
                    walk::walk_function(collector, function, ScopeFlags::Function);
                });
                true
            }
            _ => false,
        }
    }

    fn visit_finite_iteration_call(&mut self, call: &CallExpression<'_>) -> bool {
        let Some(member) = call.callee.get_member_expr() else {
            return false;
        };
        let Some(method) = member.static_property_name() else {
            return false;
        };
        if !matches!(method.as_ref(), "map" | "forEach") {
            return false;
        }
        let Some(values) = self.finite_iterable_from_expression(member.object()) else {
            return false;
        };
        let Some(callback) = call.arguments.first() else {
            return false;
        };
        self.visit_finite_iteration_callback(callback, values)
    }

    fn for_of_binding_name<'a>(&self, statement: &'a ForOfStatement<'a>) -> Option<&'a str> {
        match &statement.left {
            ForStatementLeft::VariableDeclaration(declaration) => declaration
                .declarations
                .first()
                .and_then(|declarator| binding_identifier_name(&declarator.id)),
            ForStatementLeft::AssignmentTargetIdentifier(identifier) => {
                Some(identifier.name.as_str())
            }
            _ => None,
        }
    }
}

impl<'a> Visit<'a> for HelperBodyCollector {
    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        if declarator.kind == VariableDeclarationKind::Const {
            if let Some(name) = binding_identifier_name(&declarator.id) {
                if let Some(init) = &declarator.init {
                    if let Some(values) =
                        finite_strings_from_expression(init, &self.finite_constants)
                    {
                        self.finite_constants.insert(name.to_string(), values);
                    }
                    if let Some(values) = self.finite_iterable_from_expression(init) {
                        self.finite_iterables.insert(name.to_string(), values);
                    }
                }
            }
        }
        walk::walk_variable_declarator(self, declarator);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if self.visit_finite_iteration_call(call) {
            return;
        }

        if let Some(param_index) = self.callee_param_index(&call.callee) {
            let keys = call.arguments.first().and_then(|argument| {
                finite_strings_from_argument(argument, &self.finite_constants)
            });
            self.usages.push(HelperParamUsage { param_index, keys });
        }
        walk::walk_call_expression(self, call);
    }

    fn visit_function(&mut self, _function: &Function<'a>, _flags: ScopeFlags) {}

    fn visit_arrow_function_expression(&mut self, _arrow: &ArrowFunctionExpression<'a>) {}

    fn visit_for_of_statement(&mut self, statement: &ForOfStatement<'a>) {
        let Some(values) = self.finite_iterable_from_expression(&statement.right) else {
            walk::walk_for_of_statement(self, statement);
            return;
        };
        let Some(binding) = self.for_of_binding_name(statement).map(str::to_string) else {
            walk::walk_for_of_statement(self, statement);
            return;
        };
        self.with_finite_constant(&binding, values, |collector| {
            collector.visit_statement(&statement.body);
        });
    }
}

struct ReturnValueCollector {
    finite_constants: BTreeMap<String, FiniteStrings>,
    finite_iterables: BTreeMap<String, FiniteStrings>,
    return_values: FiniteStrings,
    unknown_return: bool,
}

impl ReturnValueCollector {
    fn finite_iterable_from_expression(
        &self,
        expression: &Expression<'_>,
    ) -> Option<FiniteStrings> {
        finite_iterable_from_expression(expression, &self.finite_constants, &self.finite_iterables)
    }
}

impl<'a> Visit<'a> for ReturnValueCollector {
    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        if declarator.kind == VariableDeclarationKind::Const {
            if let Some(name) = binding_identifier_name(&declarator.id) {
                if let Some(init) = &declarator.init {
                    if let Some(values) =
                        finite_strings_from_expression(init, &self.finite_constants)
                    {
                        self.finite_constants.insert(name.to_string(), values);
                    }
                    if let Some(values) = self.finite_iterable_from_expression(init) {
                        self.finite_iterables.insert(name.to_string(), values);
                    }
                }
            }
        }
        walk::walk_variable_declarator(self, declarator);
    }

    fn visit_return_statement(&mut self, statement: &ReturnStatement<'a>) {
        let Some(argument) = &statement.argument else {
            self.unknown_return = true;
            return;
        };
        let Some(values) = finite_strings_from_expression(argument, &self.finite_constants) else {
            self.unknown_return = true;
            return;
        };
        self.return_values.extend(values);
        if self.return_values.len() > MAX_FINITE_STRINGS {
            self.unknown_return = true;
        }
    }

    fn visit_function(&mut self, _function: &Function<'a>, _flags: ScopeFlags) {}

    fn visit_arrow_function_expression(&mut self, _arrow: &ArrowFunctionExpression<'a>) {}
}

#[derive(Default)]
struct SourceIndexCollector {
    imports: BTreeMap<String, ImportTarget>,
    helpers: BTreeMap<String, HelperSummary>,
    return_helpers: BTreeMap<String, FiniteStrings>,
    export_locals: BTreeMap<String, String>,
    default_local: Option<String>,
    default_summary: Option<HelperSummary>,
    default_return_summary: Option<FiniteStrings>,
}

impl SourceIndexCollector {
    fn finish(self) -> SourceFileIndex {
        let mut named_exports = BTreeMap::new();
        let mut named_return_exports = BTreeMap::new();
        for (exported, local) in self.export_locals {
            if let Some(summary) = self.helpers.get(&local) {
                named_exports.insert(exported.clone(), summary.clone());
            }
            if let Some(summary) = self.return_helpers.get(&local) {
                named_return_exports.insert(exported, summary.clone());
            }
        }

        let default_local = self.default_local;
        let default_export = self.default_summary.or_else(|| {
            default_local
                .as_ref()
                .and_then(|name| self.helpers.get(name).cloned())
        });
        let default_return_export = self.default_return_summary.or_else(|| {
            default_local
                .as_ref()
                .and_then(|name| self.return_helpers.get(name).cloned())
        });

        SourceFileIndex {
            imports: self.imports,
            helpers: self.helpers,
            return_helpers: self.return_helpers,
            named_exports,
            named_return_exports,
            default_export,
            default_return_export,
        }
    }

    fn record_helper(&mut self, name: &str, summary: HelperSummary) {
        self.helpers.insert(name.to_string(), summary);
    }

    fn record_return_helper(&mut self, name: &str, summary: FiniteStrings) {
        self.return_helpers.insert(name.to_string(), summary);
    }

    fn record_variable_helpers(&mut self, declaration: &VariableDeclaration<'_>) {
        for declarator in &declaration.declarations {
            let Some(name) = binding_identifier_name(&declarator.id) else {
                continue;
            };
            let Some(init) = &declarator.init else {
                continue;
            };
            if let Some(summary) = helper_summary_from_expression(init) {
                self.record_helper(name, summary);
            }
            if let Some(summary) = finite_return_summary_from_expression(init) {
                self.record_return_helper(name, summary);
            }
        }
    }

    fn record_export_declaration(&mut self, declaration: &Declaration<'_>) {
        match declaration {
            Declaration::FunctionDeclaration(function) => {
                if let Some(id) = &function.id {
                    self.export_locals
                        .insert(id.name.to_string(), id.name.to_string());
                }
            }
            Declaration::VariableDeclaration(variable) => {
                for declarator in &variable.declarations {
                    if let Some(name) = binding_identifier_name(&declarator.id) {
                        self.export_locals
                            .insert(name.to_string(), name.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    fn record_export_specifiers(&mut self, declaration: &ExportNamedDeclaration<'_>) {
        if declaration.source.is_some() {
            return;
        }
        for specifier in &declaration.specifiers {
            let Some(local) = module_export_name(&specifier.local) else {
                continue;
            };
            let Some(exported) = module_export_name(&specifier.exported) else {
                continue;
            };
            self.export_locals
                .insert(exported.to_string(), local.to_string());
        }
    }

    fn record_default_export(&mut self, declaration: &ExportDefaultDeclaration<'_>) {
        match &declaration.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                if let Some(summary) = helper_summary_from_function(function) {
                    self.default_summary = Some(summary);
                }
                if let Some(summary) = finite_return_summary_from_function(function) {
                    self.default_return_summary = Some(summary);
                }
                if let Some(id) = &function.id {
                    self.record_helper(
                        id.name.as_str(),
                        self.default_summary.clone().unwrap_or_default(),
                    );
                    if let Some(summary) = self.default_return_summary.clone() {
                        self.record_return_helper(id.name.as_str(), summary);
                    }
                }
            }
            ExportDefaultDeclarationKind::Identifier(identifier) => {
                self.default_local = Some(identifier.name.to_string());
            }
            ExportDefaultDeclarationKind::ArrowFunctionExpression(arrow) => {
                self.default_summary = Some(helper_summary_from_arrow(arrow));
                self.default_return_summary = finite_return_summary_from_arrow(arrow);
            }
            ExportDefaultDeclarationKind::FunctionExpression(function) => {
                self.default_summary = helper_summary_from_function(function);
                self.default_return_summary = finite_return_summary_from_function(function);
            }
            _ => {}
        }
    }
}

impl<'a> Visit<'a> for SourceIndexCollector {
    fn visit_import_declaration(&mut self, declaration: &ImportDeclaration<'a>) {
        let source = declaration.source.value.as_str();
        if source != "next-intl" && source != "next-intl/server" {
            if let Some(specifiers) = &declaration.specifiers {
                for specifier in specifiers {
                    match specifier {
                        ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                            if let Some(imported) = module_export_name(&specifier.imported) {
                                self.imports.insert(
                                    specifier.local.name.to_string(),
                                    ImportTarget {
                                        source: source.to_string(),
                                        imported: ImportedName::Named(imported.to_string()),
                                    },
                                );
                            }
                        }
                        ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => {
                            self.imports.insert(
                                specifier.local.name.to_string(),
                                ImportTarget {
                                    source: source.to_string(),
                                    imported: ImportedName::Default,
                                },
                            );
                        }
                        ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => {}
                    }
                }
            }
        }

        walk::walk_import_declaration(self, declaration);
    }

    fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
        if let Some(id) = &function.id {
            if let Some(summary) = helper_summary_from_function(function) {
                self.record_helper(id.name.as_str(), summary);
            }
            if let Some(summary) = finite_return_summary_from_function(function) {
                self.record_return_helper(id.name.as_str(), summary);
            }
        }
        walk::walk_function(self, function, flags);
    }

    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        if let Some(name) = binding_identifier_name(&declarator.id) {
            if let Some(init) = &declarator.init {
                if let Some(summary) = helper_summary_from_expression(init) {
                    self.record_helper(name, summary);
                }
                if let Some(summary) = finite_return_summary_from_expression(init) {
                    self.record_return_helper(name, summary);
                }
            }
        }
        walk::walk_variable_declarator(self, declarator);
    }

    fn visit_export_named_declaration(&mut self, declaration: &ExportNamedDeclaration<'a>) {
        if let Some(inner) = &declaration.declaration {
            self.record_export_declaration(inner);
            if let Declaration::VariableDeclaration(variable) = inner {
                self.record_variable_helpers(variable);
            }
        }
        self.record_export_specifiers(declaration);
        walk::walk_export_named_declaration(self, declaration);
    }

    fn visit_export_default_declaration(&mut self, declaration: &ExportDefaultDeclaration<'a>) {
        self.record_default_export(declaration);
        walk::walk_export_default_declaration(self, declaration);
    }
}

impl SourceUsageCollector {
    fn new(path: &Path, source: &str, project: Option<&ProjectIndex>) -> Self {
        let project = project.cloned();
        let file_index = project
            .as_ref()
            .and_then(|project| project.files.get(path).cloned());
        Self {
            path: path.to_path_buf(),
            line_starts: line_starts(source),
            project,
            file_index,
            use_translations: BTreeSet::new(),
            get_translations: BTreeSet::new(),
            use_extracted: BTreeSet::new(),
            get_extracted: BTreeSet::new(),
            finite_constants: BTreeMap::new(),
            finite_iterables: BTreeMap::new(),
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
        self.finite_strings_from_argument(argument)
            .and_then(|values| single_finite_string(&values))
    }

    fn string_from_expression(&self, expression: &Expression<'_>) -> Option<String> {
        self.finite_strings_from_expression(expression)
            .and_then(|values| single_finite_string(&values))
    }

    fn finite_iterable_from_expression(
        &self,
        expression: &Expression<'_>,
    ) -> Option<FiniteStrings> {
        finite_iterable_from_expression(expression, &self.finite_constants, &self.finite_iterables)
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

    fn return_helper_for_callee(&self, expression: &Expression<'_>) -> Option<FiniteStrings> {
        let callee = self.callee_identifier(expression)?;
        if let Some(file_index) = &self.file_index {
            if let Some(summary) = file_index.return_helper_for_local(callee) {
                return Some(summary);
            }
            if let Some(target) = file_index.imports.get(callee) {
                if let Some(project) = &self.project {
                    return project.return_helper_for_import(&self.path, target);
                }
            }
        }
        None
    }

    fn helper_summary_for_callee(&self, expression: &Expression<'_>) -> Option<HelperSummary> {
        let callee = self.callee_identifier(expression)?;
        if let Some(file_index) = &self.file_index {
            if let Some(summary) = file_index.helper_for_local(callee) {
                return Some(summary);
            }
            if let Some(target) = file_index.imports.get(callee) {
                if let Some(project) = &self.project {
                    return project.helper_for_import(&self.path, target);
                }
            }
        }
        None
    }

    fn callee_is_potential_helper(&self, expression: &Expression<'_>) -> bool {
        let Some(callee) = self.callee_identifier(expression) else {
            return false;
        };
        if self.use_translations.contains(callee)
            || self.get_translations.contains(callee)
            || self.use_extracted.contains(callee)
            || self.get_extracted.contains(callee)
            || self.translators.contains_key(callee)
        {
            return false;
        }
        true
    }

    fn translator_binding_from_argument(
        &self,
        argument: &Argument<'_>,
    ) -> Option<TranslatorBinding> {
        match argument {
            Argument::Identifier(identifier) => {
                self.translators.get(identifier.name.as_str()).cloned()
            }
            Argument::CallExpression(call) => self.translator_binding_from_call(call),
            _ => None,
        }
    }

    fn translator_binding_from_call(&self, call: &CallExpression<'_>) -> Option<TranslatorBinding> {
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

    fn apply_helper_summary(&mut self, call: &CallExpression<'_>, summary: &HelperSummary) {
        for usage in &summary.param_usages {
            let Some(argument) = call.arguments.get(usage.param_index) else {
                continue;
            };
            let Some(binding) = self.translator_binding_from_argument(argument) else {
                continue;
            };
            match (&usage.keys, binding.dynamic_namespace) {
                (Some(keys), false) => {
                    for key in keys {
                        let id = match &binding.namespace {
                            Some(namespace) if key.is_empty() => namespace.clone(),
                            Some(namespace) => format!("{namespace}.{key}"),
                            None => key.clone(),
                        };
                        self.scan.used_ids.insert(id);
                    }
                }
                _ => self.record_dynamic_usage(binding.namespace.as_deref(), call.span.start),
            }
        }
    }

    fn protect_translator_arguments(&mut self, call: &CallExpression<'_>) {
        for argument in &call.arguments {
            if let Some(binding) = self.translator_binding_from_argument(argument) {
                self.record_dynamic_usage(binding.namespace.as_deref(), call.span.start);
            }
        }
    }

    fn finite_strings_from_argument(&self, argument: &Argument<'_>) -> Option<FiniteStrings> {
        match argument {
            Argument::CallExpression(call) => self.finite_strings_from_call(call),
            _ => finite_strings_from_argument(argument, &self.finite_constants),
        }
    }

    fn finite_strings_from_expression(&self, expression: &Expression<'_>) -> Option<FiniteStrings> {
        match expression.get_inner_expression() {
            Expression::CallExpression(call) => self.finite_strings_from_call(call),
            _ => finite_strings_from_expression(expression, &self.finite_constants),
        }
    }

    fn finite_strings_from_call(&self, call: &CallExpression<'_>) -> Option<FiniteStrings> {
        self.return_helper_for_callee(&call.callee)
            .or_else(|| finite_strings_from_call(call, &self.finite_constants))
    }

    fn with_finite_constant(
        &mut self,
        name: &str,
        values: FiniteStrings,
        visit: impl FnOnce(&mut Self),
    ) {
        let previous = self.finite_constants.insert(name.to_string(), values);
        visit(self);
        match previous {
            Some(values) => {
                self.finite_constants.insert(name.to_string(), values);
            }
            None => {
                self.finite_constants.remove(name);
            }
        }
    }

    fn visit_finite_iteration_callback(
        &mut self,
        callback: &Argument<'_>,
        values: FiniteStrings,
    ) -> bool {
        match callback {
            Argument::ArrowFunctionExpression(arrow) => {
                let Some(param) = arrow
                    .params
                    .items
                    .first()
                    .and_then(|param| binding_identifier_name(&param.pattern))
                else {
                    return false;
                };
                self.with_finite_constant(param, values, |collector| {
                    walk::walk_arrow_function_expression(collector, arrow);
                });
                true
            }
            Argument::FunctionExpression(function) => {
                let Some(param) = function
                    .params
                    .items
                    .first()
                    .and_then(|param| binding_identifier_name(&param.pattern))
                else {
                    return false;
                };
                self.with_finite_constant(param, values, |collector| {
                    walk::walk_function(collector, function, ScopeFlags::Function);
                });
                true
            }
            _ => false,
        }
    }

    fn visit_finite_iteration_call(&mut self, call: &CallExpression<'_>) -> bool {
        let Some(member) = call.callee.get_member_expr() else {
            return false;
        };
        let Some(method) = member.static_property_name() else {
            return false;
        };
        if !matches!(method.as_ref(), "map" | "forEach") {
            return false;
        }
        let Some(values) = self.finite_iterable_from_expression(member.object()) else {
            return false;
        };
        let Some(callback) = call.arguments.first() else {
            return false;
        };
        self.visit_finite_iteration_callback(callback, values)
    }

    fn for_of_binding_name<'a>(&self, statement: &'a ForOfStatement<'a>) -> Option<&'a str> {
        match &statement.left {
            ForStatementLeft::VariableDeclaration(declaration) => declaration
                .declarations
                .first()
                .and_then(|declarator| binding_identifier_name(&declarator.id)),
            ForStatementLeft::AssignmentTargetIdentifier(identifier) => {
                Some(identifier.name.as_str())
            }
            _ => None,
        }
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
                    if let Some(values) = self.finite_strings_from_expression(init) {
                        self.finite_constants.insert(name.to_string(), values);
                    }
                    if let Some(values) = self.finite_iterable_from_expression(init) {
                        self.finite_iterables.insert(name.to_string(), values);
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

        if self.visit_finite_iteration_call(call) {
            return;
        }

        if let Some(binding) = self.maybe_translator_call(&call.callee) {
            let keys = call
                .arguments
                .first()
                .and_then(|argument| self.finite_strings_from_argument(argument));
            match keys {
                Some(keys) if !binding.dynamic_namespace => {
                    for key in keys {
                        let id = match &binding.namespace {
                            Some(namespace) if key.is_empty() => namespace.clone(),
                            Some(namespace) => format!("{namespace}.{key}"),
                            None => key,
                        };
                        self.scan.used_ids.insert(id);
                    }
                }
                _ => self.record_dynamic_usage(binding.namespace.as_deref(), call.span.start),
            }
        }

        if let Some(summary) = self.helper_summary_for_callee(&call.callee) {
            self.apply_helper_summary(call, &summary);
        } else if self.callee_is_potential_helper(&call.callee) {
            self.protect_translator_arguments(call);
        }

        walk::walk_call_expression(self, call);
    }

    fn visit_for_of_statement(&mut self, statement: &ForOfStatement<'a>) {
        let Some(values) = self.finite_iterable_from_expression(&statement.right) else {
            walk::walk_for_of_statement(self, statement);
            return;
        };
        let Some(binding) = self.for_of_binding_name(statement).map(str::to_string) else {
            walk::walk_for_of_statement(self, statement);
            return;
        };
        self.with_finite_constant(&binding, values, |collector| {
            collector.visit_statement(&statement.body);
        });
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
    fn resolves_finite_conditional_keys() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations('files');
            const folderCountKey = searchAllFolders ? 'folder-count' : 'subfolder-count';
            t(folderCountKey);
            "#,
        );
        assert!(scan.used_ids.contains("files.folder-count"));
        assert!(scan.used_ids.contains("files.subfolder-count"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_finite_template_keys() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations('files');
            const suffix = compact ? 'short' : 'long';
            t(`labels.${suffix}`);
            "#,
        );
        assert!(scan.used_ids.contains("files.labels.long"));
        assert!(scan.used_ids.contains("files.labels.short"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_finite_string_transforms() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations('template');
            const type = compact ? 'NUMBER' : 'TEXT';
            t(`variables.types.${type.toLowerCase()}`);
            "#,
        );
        assert!(scan.used_ids.contains("template.variables.types.number"));
        assert!(scan.used_ids.contains("template.variables.types.text"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_finite_iterated_string_transforms() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations('template');
            const VARIABLE_TYPES = ['NUMBER', 'TEXT'] as const;
            VARIABLE_TYPES.map(type => t(`variables.types.${type.toLowerCase()}`));
            "#,
        );
        assert!(scan.used_ids.contains("template.variables.types.number"));
        assert!(scan.used_ids.contains("template.variables.types.text"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_same_file_finite_return_helper_keys() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            function getPriorityLabelKey(priority) {
              switch (priority) {
                case 'LOW':
                  return 'priority-low';
                case 'HIGH':
                  return 'priority-high';
              }
            }
            const t = useTranslations('deviations');
            t(getPriorityLabelKey(priority));
            "#,
        );
        assert!(scan.used_ids.contains("deviations.priority-low"));
        assert!(scan.used_ids.contains("deviations.priority-high"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_expression_arrow_finite_return_helper_keys() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const getKey = () => 'title';
            const t = useTranslations('settings');
            t(getKey());
            "#,
        );
        assert!(scan.used_ids.contains("settings.title"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn unknown_return_helper_stays_dynamic() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            function getKey(value) {
              if (value) return 'known';
              return value;
            }
            const t = useTranslations('settings');
            t(getKey(value));
            "#,
        );
        assert!(!scan.used_ids.contains("settings.known"));
        assert!(
            scan.dynamic_usages
                .iter()
                .any(|usage| usage.namespace == "settings")
        );
    }

    #[test]
    fn unsupported_string_transform_stays_dynamic() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations('template');
            const type = 'number';
            t(type.trim());
            "#,
        );
        assert!(
            scan.dynamic_usages
                .iter()
                .any(|usage| usage.namespace == "template")
        );
    }

    #[test]
    fn resolves_finite_map_callback_keys() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations('template');
            const TEMPLATE_TABS = ['overview', 'variables'] as const;
            TEMPLATE_TABS.map(tab => t(`tabs.${tab}.label`));
            "#,
        );
        assert!(scan.used_ids.contains("template.tabs.overview.label"));
        assert!(scan.used_ids.contains("template.tabs.variables.label"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_finite_for_each_callback_keys() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations('settings');
            (['REGULAR', 'ABSENCE'] as const).forEach(option => {
              t(`timeTypes.categories.${option}`);
            });
            "#,
        );
        assert!(
            scan.used_ids
                .contains("settings.timeTypes.categories.REGULAR")
        );
        assert!(
            scan.used_ids
                .contains("settings.timeTypes.categories.ABSENCE")
        );
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_finite_for_of_keys() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations('relation');
            const RELATIONS = ['parent', 'child'] as const;
            for (const relation of RELATIONS) {
              t(relation);
            }
            "#,
        );
        assert!(scan.used_ids.contains("relation.parent"));
        assert!(scan.used_ids.contains("relation.child"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn unresolved_iteration_stays_dynamic() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations('template');
            tabs.map(tab => t(`tabs.${tab}.label`));
            "#,
        );
        assert!(
            scan.dynamic_usages
                .iter()
                .any(|usage| usage.namespace == "template")
        );
    }

    #[test]
    fn finite_expression_unknown_branch_stays_dynamic() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations('files');
            const key = enabled ? 'known' : unknownKey;
            t(key);
            "#,
        );
        assert!(!scan.used_ids.contains("files.known"));
        assert!(
            scan.dynamic_usages
                .iter()
                .any(|usage| usage.namespace == "files")
        );
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

    #[test]
    fn traces_same_file_function_helper_by_parameter_position() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            function getMessage(tSettings) {
              return tSettings('navigation.title');
            }
            const t = useTranslations('settings');
            getMessage(t);
            "#,
        );
        assert!(scan.used_ids.contains("settings.navigation.title"));
    }

    #[test]
    fn traces_arrow_and_function_expression_helpers() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const getTitle = (translate) => translate('title');
            const getDescription = function(tx) { return tx('description'); };
            const t = useTranslations('settings');
            getTitle(t);
            getDescription(t);
            "#,
        );
        assert!(scan.used_ids.contains("settings.title"));
        assert!(scan.used_ids.contains("settings.description"));
    }

    #[test]
    fn traces_helper_method_variants_and_multiple_params() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            function helper(first, second) {
              first.rich('title');
              second.markup('body');
              second.raw('payload');
              first.has('optional');
            }
            const settings = useTranslations('settings');
            const common = useTranslations('common');
            helper(settings, common);
            "#,
        );
        assert!(scan.used_ids.contains("settings.title"));
        assert!(scan.used_ids.contains("settings.optional"));
        assert!(scan.used_ids.contains("common.body"));
        assert!(scan.used_ids.contains("common.payload"));
    }

    #[test]
    fn traces_helper_finite_conditional_keys() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            function helper(tx) {
              const key = compact ? 'short' : 'long';
              tx(key);
            }
            const t = useTranslations('settings');
            helper(t);
            "#,
        );
        assert!(scan.used_ids.contains("settings.long"));
        assert!(scan.used_ids.contains("settings.short"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn traces_helper_finite_iteration_keys() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            function helper(tx) {
              const keys = ['first', 'second'] as const;
              keys.forEach(key => tx(key));
            }
            const t = useTranslations('settings');
            helper(t);
            "#,
        );
        assert!(scan.used_ids.contains("settings.first"));
        assert!(scan.used_ids.contains("settings.second"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn helper_dynamic_key_protects_translator_namespace() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            function helper(tx, key) {
              tx(key);
            }
            const t = useTranslations('settings');
            helper(t, key);
            "#,
        );
        assert!(
            scan.dynamic_usages
                .iter()
                .any(|usage| usage.namespace == "settings")
        );
    }

    #[test]
    fn label_key_data_is_not_translation_usage() {
        let scan = scan(
            r#"
            const section = {
              labelKey: 'navigation.sections.account'
            };
            "#,
        );
        assert!(!scan.used_ids.contains("navigation.sections.account"));
    }
}
