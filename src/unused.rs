use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, ArrowFunctionExpression, BindingPattern, CallExpression,
    ComputedMemberExpression, ConditionalExpression, Declaration, ExportDefaultDeclaration,
    ExportDefaultDeclarationKind, ExportNamedDeclaration, Expression, ForOfStatement,
    ForStatementLeft, Function, FunctionBody, ImportDeclaration, ImportDeclarationSpecifier,
    ModuleExportName, ObjectExpression, ObjectPropertyKind, PropertyKind, ReturnStatement,
    Statement, StaticMemberExpression, TSInterfaceDeclaration, TSLiteral, TSSignature, TSType,
    TSTypeAliasDeclaration, TSTypeName, TSTypeOperatorOperator, TSTypeQueryExprName,
    TemplateLiteral, VariableDeclaration, VariableDeclarationKind, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};
use oxc_syntax::{operator::LogicalOperator, scope::ScopeFlags};
use serde::{Deserialize, Serialize};

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
type FiniteObjectMap = BTreeMap<String, FiniteStrings>;
type FiniteObjectMaps = BTreeMap<String, FiniteObjectMap>;
type FiniteRecords = Vec<FiniteRecord>;
type FiniteRecordBindings = BTreeMap<String, FiniteRecords>;
type FiniteRecordMaps = BTreeMap<String, FiniteRecords>;
type TypeDomains = BTreeMap<String, FiniteStrings>;
type TypePropertyDomains = BTreeMap<String, BTreeMap<String, FiniteStrings>>;
type PropertyDomains = BTreeMap<String, FiniteStrings>;

#[derive(Debug, Clone, Default)]
struct FiniteRecord {
    strings: BTreeMap<String, FiniteStrings>,
    record_iterables: BTreeMap<String, FiniteRecords>,
}

#[derive(Debug, Clone, Default)]
pub struct UnusedReport {
    pub unused_ids: Vec<String>,
    pub total_ids: usize,
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
    key_start: Option<usize>,
    key_end: Option<usize>,
}

#[derive(Debug, Clone)]
struct TranslatorBinding {
    namespace: Option<String>,
    namespaces: Option<FiniteStrings>,
    dynamic_namespace: bool,
}

#[derive(Debug, Clone)]
enum NamespaceArg {
    Scoped(String),
    Finite(FiniteStrings),
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
    return_record_helpers: BTreeMap<String, FiniteRecords>,
    named_exports: BTreeMap<String, HelperSummary>,
    named_return_exports: BTreeMap<String, FiniteStrings>,
    named_iterable_exports: BTreeMap<String, FiniteStrings>,
    named_record_iterable_exports: BTreeMap<String, FiniteRecords>,
    named_record_return_exports: BTreeMap<String, FiniteRecords>,
    default_export: Option<HelperSummary>,
    default_return_export: Option<FiniteStrings>,
    default_iterable_export: Option<FiniteStrings>,
    default_record_iterable_export: Option<FiniteRecords>,
    default_record_return_export: Option<FiniteRecords>,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TsCheckerRequest {
    root: String,
    queries: Vec<TsCheckerQuery>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TsCheckerQuery {
    index: usize,
    file: String,
    start: usize,
    end: usize,
    namespace: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TsCheckerResponse {
    results: Vec<TsCheckerResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TsCheckerResult {
    index: usize,
    keys: Vec<String>,
}

const TS_CHECKER_SCRIPT: &str = r#"
const fs = require('fs');

function readStdin() {
  return new Promise((resolve, reject) => {
    let data = '';
    process.stdin.setEncoding('utf8');
    process.stdin.on('data', chunk => { data += chunk; });
    process.stdin.on('end', () => resolve(data));
    process.stdin.on('error', reject);
  });
}

function findNode(sourceFile, start, end) {
  let best = sourceFile;
  function visit(node) {
    const nodeStart = node.getStart(sourceFile, false);
    const nodeEnd = node.getEnd();
    if (start < nodeStart || end > nodeEnd) return;
    best = node;
    node.forEachChild(visit);
  }
  visit(sourceFile);
  return best;
}

function unique(values) {
  return [...new Set(values)].sort();
}

function finiteFromType(ts, checker, type) {
  if (!type) return null;
  if (type.isUnion && type.isUnion()) {
    let values = [];
    for (const part of type.types) {
      const partValues = finiteFromType(ts, checker, part);
      if (!partValues) return null;
      values.push(...partValues);
      if (values.length > 128) return null;
    }
    return unique(values);
  }
  if (type.isStringLiteral && type.isStringLiteral()) {
    return [type.value];
  }
  if ((type.flags & ts.TypeFlags.StringLiteral) && typeof type.value === 'string') {
    return [type.value];
  }
  if (type.isLiteral && type.isLiteral() && typeof type.value === 'string') {
    return [type.value];
  }
  return null;
}

function appendFinite(left, right) {
  const values = [];
  for (const l of left) {
    for (const r of right) {
      values.push(`${l}${r}`);
      if (values.length > 128) return null;
    }
  }
  return unique(values);
}

function transform(values, method) {
  if (method === 'toLowerCase') return values.map(value => value.toLowerCase());
  if (method === 'toUpperCase') return values.map(value => value.toUpperCase());
  return null;
}

function finiteFromExpression(ts, checker, node) {
  if (!node) return null;

  if (ts.isParenthesizedExpression(node) || ts.isAsExpression(node) || ts.isSatisfiesExpression?.(node) || ts.isNonNullExpression(node) || ts.isTypeAssertionExpression(node)) {
    return finiteFromExpression(ts, checker, node.expression);
  }

  if (ts.isStringLiteralLike(node)) return [node.text];

  if (ts.isTemplateExpression(node)) {
    let values = [node.head.text];
    for (const span of node.templateSpans) {
      const expressionValues = finiteFromExpression(ts, checker, span.expression);
      if (!expressionValues) return null;
      values = appendFinite(values, expressionValues);
      if (!values) return null;
      values = appendFinite(values, [span.literal.text]);
      if (!values) return null;
    }
    return values;
  }

  if (ts.isNoSubstitutionTemplateLiteral(node)) return [node.text];

  if (ts.isCallExpression(node) && node.arguments.length === 0 && ts.isPropertyAccessExpression(node.expression)) {
    const method = node.expression.name.text;
    const objectValues = finiteFromExpression(ts, checker, node.expression.expression);
    if (!objectValues) return null;
    return transform(objectValues, method);
  }

  if (ts.isPropertyAccessExpression(node) || ts.isElementAccessExpression(node) || ts.isIdentifier(node)) {
    return finiteFromType(ts, checker, checker.getTypeAtLocation(node));
  }

  return finiteFromType(ts, checker, checker.getTypeAtLocation(node));
}

(async () => {
  const input = JSON.parse(await readStdin());
  const tsPath = require.resolve('typescript', { paths: [input.root] });
  const ts = require(tsPath);
  const configPath =
    ts.findConfigFile(input.root, ts.sys.fileExists, 'tsconfig.json') ||
    ts.findConfigFile(input.root, ts.sys.fileExists, 'jsconfig.json');
  if (!configPath) {
    console.log(JSON.stringify({ results: [] }));
    return;
  }
  const config = ts.readConfigFile(configPath, ts.sys.readFile);
  if (config.error) {
    console.log(JSON.stringify({ results: [] }));
    return;
  }
  const parsed = ts.parseJsonConfigFileContent(
    config.config,
    ts.sys,
    require('path').dirname(configPath),
    { noEmit: true, skipLibCheck: true },
    configPath,
  );
  const program = ts.createProgram({
    rootNames: parsed.fileNames,
    options: parsed.options,
  });
  const checker = program.getTypeChecker();
  const results = [];
  for (const query of input.queries) {
    const sourceFile = program.getSourceFile(query.file);
    if (!sourceFile) continue;
    const node = findNode(sourceFile, query.start, query.end);
    const keys = finiteFromExpression(ts, checker, node);
    if (keys && keys.length > 0 && keys.length <= 128) {
      results.push({ index: query.index, keys });
    }
  }
  console.log(JSON.stringify({ results }));
})().catch(() => {
  console.log(JSON.stringify({ results: [] }));
});
"#;

pub fn find_unused(root: impl AsRef<Path>, config: &TransConfig) -> Result<UnusedReport> {
    find_unused_with_options(root, config, true, true)
}

pub fn find_unused_with_ts_checker(
    root: impl AsRef<Path>,
    config: &TransConfig,
    use_ts_checker: bool,
) -> Result<UnusedReport> {
    find_unused_with_options(root, config, true, use_ts_checker)
}

pub fn find_unused_keys(root: impl AsRef<Path>, config: &TransConfig) -> Result<Vec<String>> {
    Ok(find_unused_with_options(root, config, false, true)?.unused_ids)
}

pub fn find_unused_keys_with_ts_checker(
    root: impl AsRef<Path>,
    config: &TransConfig,
    use_ts_checker: bool,
) -> Result<Vec<String>> {
    Ok(find_unused_with_options(root, config, false, use_ts_checker)?.unused_ids)
}

pub fn remove_unused(
    root: impl AsRef<Path>,
    config: &TransConfig,
    force: bool,
) -> Result<UnusedReport> {
    remove_unused_with_ts_checker(root, config, force, true)
}

pub fn remove_unused_with_ts_checker(
    root: impl AsRef<Path>,
    config: &TransConfig,
    force: bool,
    use_ts_checker: bool,
) -> Result<UnusedReport> {
    let root = root.as_ref();
    let report = find_unused_with_options(root, config, !force, use_ts_checker)?;

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
    use_ts_checker: bool,
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
    let mut scan = collect_usage_from_files(root, &source_files)?;
    if use_ts_checker {
        apply_ts_checker_fallback(root, &mut scan);
    }

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
        total_ids: primary.len(),
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

fn apply_ts_checker_fallback(root: &Path, scan: &mut UsageScan) {
    let queries = ts_checker_queries(&scan.dynamic_usages);
    if queries.is_empty() {
        return;
    }

    let request = TsCheckerRequest {
        root: root.to_string_lossy().to_string(),
        queries,
    };
    let Ok(input) = serde_json::to_vec(&request) else {
        return;
    };

    let mut child = match Command::new("node")
        .arg("-e")
        .arg(TS_CHECKER_SCRIPT)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return,
    };

    if let Some(stdin) = child.stdin.as_mut() {
        if stdin.write_all(&input).is_err() {
            return;
        }
    }

    let Ok(output) = child.wait_with_output() else {
        return;
    };
    if !output.status.success() {
        return;
    }

    let Ok(response) = serde_json::from_slice::<TsCheckerResponse>(&output.stdout) else {
        return;
    };

    let mut resolved = BTreeSet::new();
    for result in response.results {
        let Some(usage) = scan.dynamic_usages.get(result.index) else {
            continue;
        };
        if result.keys.is_empty() || result.keys.len() > 128 {
            continue;
        }
        for key in &result.keys {
            let Some(id) = resolved_translation_id(&usage.namespace, key) else {
                continue;
            };
            scan.used_ids.insert(id);
        }
        resolved.insert(result.index);
    }

    if !resolved.is_empty() {
        let mut index = 0;
        scan.dynamic_usages.retain(|_| {
            let keep = !resolved.contains(&index);
            index += 1;
            keep
        });
    }
}

fn ts_checker_queries(dynamic_usages: &[DynamicUsage]) -> Vec<TsCheckerQuery> {
    dynamic_usages
        .iter()
        .enumerate()
        .filter_map(|(index, usage)| {
            let start = usage.key_start?;
            let end = usage.key_end?;
            Some(TsCheckerQuery {
                index,
                file: usage.path.to_string_lossy().to_string(),
                start,
                end,
                namespace: usage.namespace.clone(),
            })
        })
        .collect()
}

fn resolved_translation_id(namespace: &str, key: &str) -> Option<String> {
    match (namespace.is_empty(), key.is_empty()) {
        (true, true) => None,
        (true, false) => Some(key.to_string()),
        (false, true) => Some(namespace.to_string()),
        (false, false) => Some(format!("{namespace}.{key}")),
    }
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

    fn finite_iterable_for_import(
        &self,
        from: &Path,
        target: &ImportTarget,
    ) -> Option<FiniteStrings> {
        let path = self.resolve_module(from, &target.source)?;
        let file = self.files.get(&path)?;
        match &target.imported {
            ImportedName::Named(name) => file.named_iterable_exports.get(name).cloned(),
            ImportedName::Default => file.default_iterable_export.clone(),
        }
    }

    fn finite_record_iterable_for_import(
        &self,
        from: &Path,
        target: &ImportTarget,
    ) -> Option<FiniteRecords> {
        let path = self.resolve_module(from, &target.source)?;
        let file = self.files.get(&path)?;
        match &target.imported {
            ImportedName::Named(name) => file.named_record_iterable_exports.get(name).cloned(),
            ImportedName::Default => file.default_record_iterable_export.clone(),
        }
    }

    fn return_record_helper_for_import(
        &self,
        from: &Path,
        target: &ImportTarget,
    ) -> Option<FiniteRecords> {
        let path = self.resolve_module(from, &target.source)?;
        let file = self.files.get(&path)?;
        match &target.imported {
            ImportedName::Named(name) => file.named_record_return_exports.get(name).cloned(),
            ImportedName::Default => file.default_record_return_export.clone(),
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

    fn return_record_helper_for_local(&self, name: &str) -> Option<FiniteRecords> {
        self.return_record_helpers.get(name).cloned()
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
    finite_object_maps: FiniteObjectMaps,
    finite_record_constants: FiniteRecordBindings,
    finite_record_iterables: FiniteRecordBindings,
    finite_record_maps: FiniteRecordMaps,
    typed_object_property_domains: TypePropertyDomains,
    type_domains: TypeDomains,
    type_property_domains: TypePropertyDomains,
    zod_schema_property_domains: TypePropertyDomains,
    enum_member_domains: TypeDomains,
    translators: BTreeMap<String, TranslatorBinding>,
    message_key_helpers: BTreeMap<String, usize>,
    scan: UsageScan,
}

fn helper_summary_from_expression_with_context(
    expression: &Expression<'_>,
    helpers: &BTreeMap<String, HelperSummary>,
    finite_constants: &BTreeMap<String, FiniteStrings>,
    finite_iterables: &BTreeMap<String, FiniteStrings>,
    finite_object_maps: &FiniteObjectMaps,
    finite_record_constants: &FiniteRecordBindings,
    finite_record_iterables: &FiniteRecordBindings,
    finite_record_maps: &FiniteRecordMaps,
    enum_member_domains: &TypeDomains,
) -> Option<HelperSummary> {
    match expression.get_inner_expression() {
        Expression::ArrowFunctionExpression(arrow) => Some(helper_summary_from_arrow_with_context(
            arrow,
            helpers,
            finite_constants,
            finite_iterables,
            finite_object_maps,
            finite_record_constants,
            finite_record_iterables,
            finite_record_maps,
            enum_member_domains,
        )),
        Expression::FunctionExpression(function) => helper_summary_from_function_with_context(
            function,
            helpers,
            finite_constants,
            finite_iterables,
            finite_object_maps,
            finite_record_constants,
            finite_record_iterables,
            finite_record_maps,
            enum_member_domains,
        ),
        _ => None,
    }
}

fn helper_summary_from_arrow_with_context(
    arrow: &ArrowFunctionExpression<'_>,
    helpers: &BTreeMap<String, HelperSummary>,
    finite_constants: &BTreeMap<String, FiniteStrings>,
    finite_iterables: &BTreeMap<String, FiniteStrings>,
    finite_object_maps: &FiniteObjectMaps,
    finite_record_constants: &FiniteRecordBindings,
    finite_record_iterables: &FiniteRecordBindings,
    finite_record_maps: &FiniteRecordMaps,
    enum_member_domains: &TypeDomains,
) -> HelperSummary {
    helper_summary_from_body_with_context(
        &arrow.params,
        &arrow.body,
        helpers,
        finite_constants,
        finite_iterables,
        finite_object_maps,
        finite_record_constants,
        finite_record_iterables,
        finite_record_maps,
        enum_member_domains,
    )
}

fn helper_summary_from_function_with_context(
    function: &Function<'_>,
    helpers: &BTreeMap<String, HelperSummary>,
    finite_constants: &BTreeMap<String, FiniteStrings>,
    finite_iterables: &BTreeMap<String, FiniteStrings>,
    finite_object_maps: &FiniteObjectMaps,
    finite_record_constants: &FiniteRecordBindings,
    finite_record_iterables: &FiniteRecordBindings,
    finite_record_maps: &FiniteRecordMaps,
    enum_member_domains: &TypeDomains,
) -> Option<HelperSummary> {
    let body = function.body.as_ref()?;
    Some(helper_summary_from_body_with_context(
        &function.params,
        body,
        helpers,
        finite_constants,
        finite_iterables,
        finite_object_maps,
        finite_record_constants,
        finite_record_iterables,
        finite_record_maps,
        enum_member_domains,
    ))
}

fn helper_summary_from_body_with_context(
    params: &oxc_ast::ast::FormalParameters<'_>,
    body: &FunctionBody<'_>,
    helpers: &BTreeMap<String, HelperSummary>,
    finite_constants: &BTreeMap<String, FiniteStrings>,
    finite_iterables: &BTreeMap<String, FiniteStrings>,
    finite_object_maps: &FiniteObjectMaps,
    finite_record_constants: &FiniteRecordBindings,
    finite_record_iterables: &FiniteRecordBindings,
    finite_record_maps: &FiniteRecordMaps,
    enum_member_domains: &TypeDomains,
) -> HelperSummary {
    let mut param_names = BTreeMap::new();
    for (index, parameter) in params.items.iter().enumerate() {
        if let Some(name) = binding_identifier_name(&parameter.pattern) {
            param_names.insert(name.to_string(), index);
        }
    }

    let mut collector = HelperBodyCollector {
        param_names,
        helpers: helpers.clone(),
        finite_constants: finite_constants.clone(),
        finite_iterables: finite_iterables.clone(),
        finite_object_maps: finite_object_maps.clone(),
        finite_record_constants: finite_record_constants.clone(),
        finite_record_iterables: finite_record_iterables.clone(),
        finite_record_maps: finite_record_maps.clone(),
        enum_member_domains: enum_member_domains.clone(),
        usages: Vec::new(),
    };
    for statement in &body.statements {
        collector.visit_statement(statement);
    }

    HelperSummary {
        param_usages: collector.usages,
    }
}

fn message_key_helper_from_expression(expression: &Expression<'_>) -> Option<usize> {
    match expression.get_inner_expression() {
        Expression::ArrowFunctionExpression(arrow) => {
            Some(message_key_helper_from_body(&arrow.params, &arrow.body)?)
        }
        Expression::FunctionExpression(function) => message_key_helper_from_function(function),
        _ => None,
    }
}

fn message_key_helper_from_function(function: &Function<'_>) -> Option<usize> {
    let body = function.body.as_ref()?;
    Some(message_key_helper_from_body(&function.params, body)?)
}

fn message_key_helper_from_body(
    params: &oxc_ast::ast::FormalParameters<'_>,
    body: &FunctionBody<'_>,
) -> Option<usize> {
    let mut param_names = BTreeMap::new();
    for (index, parameter) in params.items.iter().enumerate() {
        if let Some(name) = binding_identifier_name(&parameter.pattern) {
            param_names.insert(name.to_string(), index);
        }
    }

    let mut collector = MessageKeyHelperCollector {
        param_names,
        param_indexes: BTreeSet::new(),
    };
    for statement in &body.statements {
        collector.visit_statement(statement);
    }
    single_usize(&collector.param_indexes)
}

fn finite_return_summary_from_expression(expression: &Expression<'_>) -> Option<FiniteStrings> {
    match expression.get_inner_expression() {
        Expression::ArrowFunctionExpression(arrow) => finite_return_summary_from_arrow(arrow),
        Expression::FunctionExpression(function) => finite_return_summary_from_function(function),
        _ => None,
    }
}

fn finite_record_return_summary_from_expression(
    expression: &Expression<'_>,
) -> Option<FiniteRecords> {
    match expression.get_inner_expression() {
        Expression::ArrowFunctionExpression(arrow) => {
            finite_record_return_summary_from_arrow(arrow)
        }
        Expression::FunctionExpression(function) => {
            finite_record_return_summary_from_function(function)
        }
        _ => None,
    }
}

fn finite_record_return_summary_from_arrow(
    arrow: &ArrowFunctionExpression<'_>,
) -> Option<FiniteRecords> {
    if arrow.expression {
        if let Some(Statement::ExpressionStatement(statement)) = arrow.body.statements.first() {
            return finite_record_from_expression(
                &statement.expression,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &BTreeMap::new(),
                &BTreeMap::new(),
                &BTreeMap::new(),
            )
            .or_else(|| {
                finite_record_iterable_from_expression(
                    &statement.expression,
                    &BTreeMap::new(),
                    &BTreeMap::new(),
                    &BTreeMap::new(),
                    &BTreeMap::new(),
                    &BTreeMap::new(),
                )
            });
        }
    }
    finite_record_return_summary_from_body(
        &arrow.body,
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
    )
}

fn finite_record_return_summary_from_function(function: &Function<'_>) -> Option<FiniteRecords> {
    let body = function.body.as_ref()?;
    finite_record_return_summary_from_body(
        body,
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
    )
}

fn finite_record_return_summary_from_function_with_context(
    function: &Function<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    iterables: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    record_constants: &FiniteRecordBindings,
    record_iterables: &FiniteRecordBindings,
    record_maps: &FiniteRecordMaps,
    enum_member_domains: &TypeDomains,
) -> Option<FiniteRecords> {
    let body = function.body.as_ref()?;
    finite_record_return_summary_from_body(
        body,
        constants.clone(),
        iterables.clone(),
        object_maps.clone(),
        record_constants.clone(),
        record_iterables.clone(),
        record_maps.clone(),
        enum_member_domains.clone(),
    )
}

fn finite_record_return_summary_from_body(
    body: &FunctionBody<'_>,
    finite_constants: BTreeMap<String, FiniteStrings>,
    finite_iterables: BTreeMap<String, FiniteStrings>,
    finite_object_maps: FiniteObjectMaps,
    finite_record_constants: FiniteRecordBindings,
    finite_record_iterables: FiniteRecordBindings,
    finite_record_maps: FiniteRecordMaps,
    enum_member_domains: TypeDomains,
) -> Option<FiniteRecords> {
    let mut collector = ReturnRecordCollector {
        finite_constants,
        finite_iterables,
        finite_object_maps,
        finite_record_constants,
        finite_record_iterables,
        finite_record_maps,
        enum_member_domains,
        return_records: Vec::new(),
        unknown_return: false,
    };
    for statement in &body.statements {
        collector.visit_statement(statement);
    }
    if collector.unknown_return || collector.return_records.is_empty() {
        None
    } else {
        Some(collector.return_records)
    }
}

fn finite_return_summary_from_arrow(arrow: &ArrowFunctionExpression<'_>) -> Option<FiniteStrings> {
    if arrow.expression {
        if let Some(Statement::ExpressionStatement(statement)) = arrow.body.statements.first() {
            return finite_strings_from_expression(
                &statement.expression,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &BTreeMap::new(),
            );
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
        finite_object_maps: BTreeMap::new(),
        enum_member_domains: BTreeMap::new(),
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

fn namespace_arg_from_finite_strings(values: FiniteStrings) -> NamespaceArg {
    let mut values = values.into_iter();
    let Some(first) = values.next() else {
        return NamespaceArg::Dynamic;
    };
    let Some(second) = values.next() else {
        return NamespaceArg::Scoped(first);
    };
    let mut namespaces = finite_string(first);
    namespaces.insert(second);
    namespaces.extend(values);
    NamespaceArg::Finite(namespaces)
}

fn translator_binding_from_namespace_arg(namespace: NamespaceArg) -> TranslatorBinding {
    match namespace {
        NamespaceArg::Scoped(namespace) => TranslatorBinding {
            namespace: Some(namespace),
            namespaces: None,
            dynamic_namespace: false,
        },
        NamespaceArg::Finite(namespaces) => TranslatorBinding {
            namespace: None,
            namespaces: Some(namespaces),
            dynamic_namespace: false,
        },
        NamespaceArg::Unscoped => TranslatorBinding {
            namespace: None,
            namespaces: None,
            dynamic_namespace: false,
        },
        NamespaceArg::Dynamic => TranslatorBinding {
            namespace: None,
            namespaces: None,
            dynamic_namespace: true,
        },
    }
}

fn module_export_name<'a>(name: &'a ModuleExportName<'a>) -> Option<&'a str> {
    match name {
        ModuleExportName::IdentifierName(identifier) => Some(identifier.name.as_str()),
        ModuleExportName::IdentifierReference(identifier) => Some(identifier.name.as_str()),
        ModuleExportName::StringLiteral(literal) => Some(literal.value.as_str()),
    }
}

fn ts_type_name_identifier<'a>(name: &'a TSTypeName<'a>) -> Option<&'a str> {
    match name {
        TSTypeName::IdentifierReference(identifier) => Some(identifier.name.as_str()),
        _ => None,
    }
}

fn ts_type_name_parts(name: &TSTypeName<'_>) -> Option<Vec<String>> {
    match name {
        TSTypeName::IdentifierReference(identifier) => Some(vec![identifier.name.to_string()]),
        TSTypeName::QualifiedName(qualified) => {
            let mut parts = ts_type_name_parts(&qualified.left)?;
            parts.push(qualified.right.name.to_string());
            Some(parts)
        }
        _ => None,
    }
}

fn ts_type_query_expr_identifier<'a>(name: &'a TSTypeQueryExprName<'a>) -> Option<&'a str> {
    match name {
        TSTypeQueryExprName::IdentifierReference(identifier) => Some(identifier.name.as_str()),
        _ => None,
    }
}

fn is_identifier_expression(expression: &Expression<'_>, expected: &str) -> bool {
    expression
        .get_identifier_reference()
        .is_some_and(|identifier| identifier.name == expected)
}

fn singularize_constant_word(word: &str) -> String {
    if word.ends_with("ies") && word.len() > 3 {
        format!("{}y", &word[..word.len() - 3])
    } else if word.ends_with("ses") && word.len() > 3 {
        word[..word.len() - 2].to_string()
    } else if word.ends_with('s') && word.len() > 1 {
        word[..word.len() - 1].to_string()
    } else {
        word.to_string()
    }
}

fn lower_camel(words: &[String]) -> String {
    let Some((first, rest)) = words.split_first() else {
        return String::new();
    };
    let mut value = first.clone();
    for word in rest {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            value.push(first.to_ascii_uppercase());
            value.extend(chars);
        }
    }
    value
}

fn property_names_from_value_constant_name(name: &str) -> BTreeSet<String> {
    let mut words: Vec<String> = name
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| part.to_ascii_lowercase())
        .collect();
    if words.is_empty() {
        return BTreeSet::new();
    }

    if matches!(
        words.last().map(String::as_str),
        Some("values" | "options" | "items")
    ) {
        words.pop();
    }

    let words: Vec<String> = words
        .into_iter()
        .map(|word| singularize_constant_word(&word))
        .collect();
    let mut properties = BTreeSet::new();
    for start in 0..words.len() {
        properties.insert(lower_camel(&words[start..]));
    }
    properties
}

fn finite_strings_from_ts_type(
    ty: &TSType<'_>,
    type_domains: &TypeDomains,
    enum_member_domains: &TypeDomains,
) -> Option<FiniteStrings> {
    match ty {
        TSType::TSLiteralType(literal_type) => match &literal_type.literal {
            TSLiteral::StringLiteral(literal) => Some(finite_string(literal.value.to_string())),
            TSLiteral::TemplateLiteral(literal) if literal.quasis.len() == 1 => literal.quasis[0]
                .value
                .cooked
                .map(|value| finite_string(value.to_string())),
            _ => None,
        },
        TSType::TSUnionType(union) => {
            let mut values = BTreeSet::new();
            for ty in &union.types {
                values.extend(finite_strings_from_ts_type(
                    ty,
                    type_domains,
                    enum_member_domains,
                )?);
                if values.len() > MAX_FINITE_STRINGS {
                    return None;
                }
            }
            Some(values)
        }
        TSType::TSParenthesizedType(parenthesized) => finite_strings_from_ts_type(
            &parenthesized.type_annotation,
            type_domains,
            enum_member_domains,
        ),
        TSType::TSTypeReference(reference) => {
            let name = ts_type_name_identifier(&reference.type_name)?;
            type_domains
                .get(name)
                .cloned()
                .or_else(|| enum_member_domains.get(name).cloned())
        }
        TSType::TSTypeOperatorType(operator)
            if operator.operator == TSTypeOperatorOperator::Readonly =>
        {
            finite_strings_from_ts_type(
                &operator.type_annotation,
                type_domains,
                enum_member_domains,
            )
        }
        _ => None,
    }
}

fn finite_iterable_from_ts_type(
    ty: &TSType<'_>,
    type_domains: &TypeDomains,
    enum_member_domains: &TypeDomains,
) -> Option<FiniteStrings> {
    match ty {
        TSType::TSArrayType(array) => {
            finite_strings_from_ts_type(&array.element_type, type_domains, enum_member_domains)
        }
        TSType::TSTypeOperatorType(operator)
            if operator.operator == TSTypeOperatorOperator::Readonly =>
        {
            finite_iterable_from_ts_type(
                &operator.type_annotation,
                type_domains,
                enum_member_domains,
            )
            .or_else(|| {
                finite_strings_from_ts_type(
                    &operator.type_annotation,
                    type_domains,
                    enum_member_domains,
                )
            })
        }
        TSType::TSTypeReference(reference) => {
            let name = ts_type_name_identifier(&reference.type_name)?;
            if matches!(name, "Array" | "ReadonlyArray" | "Readonly") {
                let first = reference.type_arguments.as_ref()?.params.first()?;
                finite_strings_from_ts_type(first, type_domains, enum_member_domains).or_else(
                    || finite_iterable_from_ts_type(first, type_domains, enum_member_domains),
                )
            } else {
                None
            }
        }
        _ => None,
    }
}

fn property_domains_from_type_literal(
    members: &[TSSignature<'_>],
    _type_property_domains: &TypePropertyDomains,
    type_domains: &TypeDomains,
    enum_member_domains: &TypeDomains,
) -> PropertyDomains {
    let mut properties = BTreeMap::new();
    for member in members {
        let TSSignature::TSPropertySignature(property) = member else {
            continue;
        };
        if property.computed {
            continue;
        }
        let Some(name) = property.key.static_name() else {
            continue;
        };
        let Some(annotation) = &property.type_annotation else {
            continue;
        };
        if let Some(values) = finite_iterable_from_ts_type(
            &annotation.type_annotation,
            type_domains,
            enum_member_domains,
        )
        .or_else(|| {
            finite_strings_from_ts_type(
                &annotation.type_annotation,
                type_domains,
                enum_member_domains,
            )
        }) {
            properties.insert(name.to_string(), values);
        }
    }
    properties
}

fn type_literal_property_annotation<'a>(
    members: &'a [TSSignature<'a>],
    property_name: &str,
) -> Option<&'a TSType<'a>> {
    for member in members {
        let TSSignature::TSPropertySignature(property) = member else {
            continue;
        };
        if property.computed {
            continue;
        }
        if property
            .key
            .static_name()
            .is_some_and(|name| name == property_name)
        {
            return property
                .type_annotation
                .as_ref()
                .map(|annotation| &annotation.type_annotation);
        }
    }
    None
}

fn merge_property_domains(
    left: PropertyDomains,
    right: PropertyDomains,
) -> Option<PropertyDomains> {
    let mut merged = left;
    for (property, values) in right {
        let entry = merged.entry(property).or_default();
        entry.extend(values);
        if entry.len() > MAX_FINITE_STRINGS {
            return None;
        }
    }
    Some(merged)
}

fn property_domains_from_ts_type(
    ty: &TSType<'_>,
    type_property_domains: &TypePropertyDomains,
    type_domains: &TypeDomains,
    enum_member_domains: &TypeDomains,
    zod_schema_property_domains: &TypePropertyDomains,
) -> Option<PropertyDomains> {
    match ty {
        TSType::TSTypeLiteral(literal) => Some(property_domains_from_type_literal(
            &literal.members,
            type_property_domains,
            type_domains,
            enum_member_domains,
        ))
        .filter(|properties| !properties.is_empty()),
        TSType::TSUnionType(union) => {
            let mut merged = BTreeMap::new();
            for ty in &union.types {
                if let Some(properties) = property_domains_from_ts_type(
                    ty,
                    type_property_domains,
                    type_domains,
                    enum_member_domains,
                    zod_schema_property_domains,
                ) {
                    merged = merge_property_domains(merged, properties)?;
                }
            }
            Some(merged).filter(|properties| !properties.is_empty())
        }
        TSType::TSParenthesizedType(parenthesized) => property_domains_from_ts_type(
            &parenthesized.type_annotation,
            type_property_domains,
            type_domains,
            enum_member_domains,
            zod_schema_property_domains,
        ),
        TSType::TSTypeReference(reference) => {
            if ts_type_name_parts(&reference.type_name)
                .is_some_and(|parts| parts.len() == 2 && parts[0] == "z" && parts[1] == "infer")
            {
                let first = reference.type_arguments.as_ref()?.params.first()?;
                let TSType::TSTypeQuery(query) = first else {
                    return None;
                };
                let schema_name = ts_type_query_expr_identifier(&query.expr_name)?;
                return zod_schema_property_domains.get(schema_name).cloned();
            }

            let name = ts_type_name_identifier(&reference.type_name)?;
            if matches!(name, "UseFormReturn" | "Partial" | "Required" | "Readonly") {
                let first = reference.type_arguments.as_ref()?.params.first()?;
                return property_domains_from_ts_type(
                    first,
                    type_property_domains,
                    type_domains,
                    enum_member_domains,
                    zod_schema_property_domains,
                );
            }
            type_property_domains.get(name).cloned()
        }
        TSType::TSTypeOperatorType(operator)
            if operator.operator == TSTypeOperatorOperator::Readonly =>
        {
            property_domains_from_ts_type(
                &operator.type_annotation,
                type_property_domains,
                type_domains,
                enum_member_domains,
                zod_schema_property_domains,
            )
        }
        _ => None,
    }
}

fn record_enum_member_domain(
    enum_member_domains: &mut TypeDomains,
    object: &Expression<'_>,
    property: &str,
) {
    let Some(object) = object.get_inner_expression().get_identifier_reference() else {
        return;
    };
    if !object
        .name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_uppercase())
    {
        return;
    }
    enum_member_domains
        .entry(object.name.to_string())
        .or_default()
        .insert(property.to_string());
}

struct EnumMemberDomainCollector<'m> {
    domains: &'m mut TypeDomains,
}

impl<'a> Visit<'a> for EnumMemberDomainCollector<'_> {
    fn visit_static_member_expression(&mut self, member: &StaticMemberExpression<'a>) {
        record_enum_member_domain(self.domains, &member.object, member.property.name.as_str());
        walk::walk_static_member_expression(self, member);
    }
}

fn record_enum_member_domains_from_expression(
    expression: &Expression<'_>,
    enum_member_domains: &mut TypeDomains,
) {
    let mut collector = EnumMemberDomainCollector {
        domains: enum_member_domains,
    };
    collector.visit_expression(expression);
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

fn single_usize(values: &BTreeSet<usize>) -> Option<usize> {
    if values.len() == 1 {
        values.first().copied()
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
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
) -> Option<FiniteStrings> {
    match argument {
        Argument::StringLiteral(literal) => Some(finite_string(literal.value.to_string())),
        Argument::TemplateLiteral(literal) => {
            finite_strings_from_template(literal, constants, object_maps, enum_member_domains)
        }
        Argument::Identifier(identifier) => constants.get(identifier.name.as_str()).cloned(),
        Argument::CallExpression(call) => {
            finite_strings_from_call(call, constants, object_maps, enum_member_domains)
        }
        Argument::ComputedMemberExpression(member) => {
            finite_strings_from_computed_member(member, constants, object_maps, enum_member_domains)
        }
        Argument::StaticMemberExpression(member) => {
            finite_strings_from_static_member(member, object_maps, enum_member_domains)
        }
        Argument::ConditionalExpression(conditional) => finite_strings_from_conditional(
            conditional,
            constants,
            object_maps,
            enum_member_domains,
        ),
        Argument::ParenthesizedExpression(parenthesized) => finite_strings_from_expression(
            &parenthesized.expression,
            constants,
            object_maps,
            enum_member_domains,
        ),
        Argument::TSAsExpression(expression) => finite_strings_from_expression(
            &expression.expression,
            constants,
            object_maps,
            enum_member_domains,
        ),
        Argument::TSSatisfiesExpression(expression) => finite_strings_from_expression(
            &expression.expression,
            constants,
            object_maps,
            enum_member_domains,
        ),
        Argument::TSNonNullExpression(expression) => finite_strings_from_expression(
            &expression.expression,
            constants,
            object_maps,
            enum_member_domains,
        ),
        Argument::TSInstantiationExpression(expression) => finite_strings_from_expression(
            &expression.expression,
            constants,
            object_maps,
            enum_member_domains,
        ),
        Argument::TSTypeAssertion(expression) => finite_strings_from_expression(
            &expression.expression,
            constants,
            object_maps,
            enum_member_domains,
        ),
        _ => None,
    }
}

fn finite_strings_from_expression(
    expression: &Expression<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
) -> Option<FiniteStrings> {
    match expression.get_inner_expression() {
        Expression::StringLiteral(literal) => Some(finite_string(literal.value.to_string())),
        Expression::TemplateLiteral(literal) => {
            finite_strings_from_template(literal, constants, object_maps, enum_member_domains)
        }
        Expression::Identifier(identifier) => constants.get(identifier.name.as_str()).cloned(),
        Expression::CallExpression(call) => {
            finite_strings_from_call(call, constants, object_maps, enum_member_domains)
        }
        Expression::ComputedMemberExpression(member) => {
            finite_strings_from_computed_member(member, constants, object_maps, enum_member_domains)
        }
        Expression::StaticMemberExpression(member) => {
            finite_strings_from_static_member(member, object_maps, enum_member_domains)
        }
        Expression::ConditionalExpression(conditional) => finite_strings_from_conditional(
            conditional,
            constants,
            object_maps,
            enum_member_domains,
        ),
        _ => None,
    }
}

fn finite_iterable_from_expression(
    expression: &Expression<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    iterables: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
) -> Option<FiniteStrings> {
    match expression.get_inner_expression() {
        Expression::ArrayExpression(array) => finite_iterable_from_array_elements(
            array.elements.iter(),
            constants,
            iterables,
            object_maps,
            enum_member_domains,
        ),
        Expression::Identifier(identifier) => iterables.get(identifier.name.as_str()).cloned(),
        _ => None,
    }
}

fn finite_iterable_from_array_elements<'a>(
    elements: impl Iterator<Item = &'a ArrayExpressionElement<'a>>,
    constants: &BTreeMap<String, FiniteStrings>,
    iterables: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
) -> Option<FiniteStrings> {
    let mut values = BTreeSet::new();
    for element in elements {
        let element_values = finite_strings_from_array_element(
            element,
            constants,
            iterables,
            object_maps,
            enum_member_domains,
        )?;
        values.extend(element_values);
        if values.len() > MAX_FINITE_STRINGS {
            return None;
        }
    }
    Some(values)
}

fn finite_object_map_from_expression(
    expression: &Expression<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
) -> Option<FiniteObjectMap> {
    match expression.get_inner_expression() {
        Expression::ObjectExpression(object) => {
            finite_object_map_from_object(object, constants, object_maps, enum_member_domains)
        }
        _ => None,
    }
}

fn finite_object_map_from_object(
    object: &ObjectExpression<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
) -> Option<FiniteObjectMap> {
    let mut map = BTreeMap::new();
    for property in &object.properties {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            return None;
        };
        if property.kind != PropertyKind::Init || property.method || property.shorthand {
            return None;
        }
        let key = property.key.static_name()?.to_string();
        let values = finite_strings_from_expression(
            &property.value,
            constants,
            object_maps,
            enum_member_domains,
        )?;
        map.insert(key, values);
        if map.len() > MAX_FINITE_STRINGS {
            return None;
        }
    }
    if map.is_empty() {
        return None;
    }
    Some(map)
}

fn finite_strings_from_computed_member(
    member: &ComputedMemberExpression<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
) -> Option<FiniteStrings> {
    let object_name = member
        .object
        .get_inner_expression()
        .get_identifier_reference()?
        .name
        .as_str();
    let map = object_maps.get(object_name)?;
    if let Some(keys) = finite_strings_from_expression(
        &member.expression,
        constants,
        object_maps,
        enum_member_domains,
    ) {
        return finite_object_map_values_for_keys(map, keys);
    }
    finite_object_map_all_values(map)
}

fn finite_strings_from_static_member(
    member: &StaticMemberExpression<'_>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
) -> Option<FiniteStrings> {
    let object_name = member
        .object
        .get_inner_expression()
        .get_identifier_reference()?
        .name
        .as_str();
    object_maps
        .get(object_name)
        .and_then(|map| map.get(member.property.name.as_str()).cloned())
        .or_else(|| {
            enum_member_domains
                .get(object_name)
                .filter(|values| values.contains(member.property.name.as_str()))
                .map(|_| finite_string(member.property.name.to_string()))
        })
}

fn finite_object_map_values_for_keys(
    map: &FiniteObjectMap,
    keys: FiniteStrings,
) -> Option<FiniteStrings> {
    let mut values = BTreeSet::new();
    for key in keys {
        values.extend(map.get(&key)?.iter().cloned());
        if values.len() > MAX_FINITE_STRINGS {
            return None;
        }
    }
    Some(values)
}

fn finite_object_map_all_values(map: &FiniteObjectMap) -> Option<FiniteStrings> {
    let mut values = BTreeSet::new();
    for entry_values in map.values() {
        values.extend(entry_values.iter().cloned());
        if values.len() > MAX_FINITE_STRINGS {
            return None;
        }
    }
    Some(values)
}

fn finite_record_property_strings(
    records: &FiniteRecords,
    property: &str,
) -> Option<FiniteStrings> {
    let mut values = BTreeSet::new();
    for record in records {
        if let Some(record_values) = record.strings.get(property) {
            values.extend(record_values.iter().cloned());
            if values.len() > MAX_FINITE_STRINGS {
                return None;
            }
        }
    }
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

fn finite_record_property_iterable(
    records: &FiniteRecords,
    property: &str,
) -> Option<FiniteRecords> {
    let mut values = Vec::new();
    for record in records {
        if let Some(record_values) = record.record_iterables.get(property) {
            values.extend(record_values.iter().cloned());
            if values.len() > MAX_FINITE_STRINGS {
                return None;
            }
        }
    }
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

fn call_is_callback_value_wrapper(call: &CallExpression<'_>, name: &str) -> bool {
    if call
        .callee
        .get_identifier_reference()
        .is_some_and(|identifier| identifier.name.as_str() == name)
    {
        return true;
    }

    call.callee
        .get_member_expr()
        .and_then(|member| member.static_property_name())
        .is_some_and(|property| property == name)
}

fn finite_records_from_callback_argument(
    callback: &Argument<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    iterables: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
    record_iterables: &FiniteRecordBindings,
    record_maps: &FiniteRecordMaps,
) -> Option<FiniteRecords> {
    match callback {
        Argument::ArrowFunctionExpression(arrow) => {
            if arrow.expression {
                if let Some(Statement::ExpressionStatement(statement)) =
                    arrow.body.statements.first()
                {
                    return finite_record_iterable_from_expression(
                        &statement.expression,
                        constants,
                        object_maps,
                        enum_member_domains,
                        record_iterables,
                        record_maps,
                    )
                    .or_else(|| {
                        finite_record_from_expression(
                            &statement.expression,
                            constants,
                            object_maps,
                            enum_member_domains,
                            record_iterables,
                            record_maps,
                        )
                    });
                }
            }
            finite_record_return_summary_from_body(
                &arrow.body,
                constants.clone(),
                iterables.clone(),
                object_maps.clone(),
                BTreeMap::new(),
                record_iterables.clone(),
                record_maps.clone(),
                enum_member_domains.clone(),
            )
        }
        Argument::FunctionExpression(function) => finite_record_return_summary_from_body(
            function.body.as_ref()?,
            constants.clone(),
            iterables.clone(),
            object_maps.clone(),
            BTreeMap::new(),
            record_iterables.clone(),
            record_maps.clone(),
            enum_member_domains.clone(),
        ),
        _ => None,
    }
}

fn finite_records_from_object_values(
    object: &ObjectExpression<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
    record_iterables: &FiniteRecordBindings,
    record_maps: &FiniteRecordMaps,
) -> Option<FiniteRecords> {
    let mut records = Vec::new();
    for property in &object.properties {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            return None;
        };
        if property.kind != PropertyKind::Init || property.method || property.shorthand {
            return None;
        }
        records.extend(finite_record_from_expression(
            &property.value,
            constants,
            object_maps,
            enum_member_domains,
            record_iterables,
            record_maps,
        )?);
        if records.len() > MAX_FINITE_STRINGS {
            return None;
        }
    }
    Some(records).filter(|records| !records.is_empty())
}

fn merge_finite_records(mut left: FiniteRecords, right: FiniteRecords) -> Option<FiniteRecords> {
    left.extend(right);
    if left.len() > MAX_FINITE_STRINGS {
        None
    } else {
        Some(left)
    }
}

fn argument_usize(argument: &Argument<'_>) -> Option<usize> {
    let value = match argument {
        Argument::NumericLiteral(literal) => literal.value,
        _ => return None,
    };
    if value.is_finite() && value >= 0.0 && value.fract() == 0.0 {
        Some(value as usize)
    } else {
        None
    }
}

fn finite_record_from_object(
    object: &ObjectExpression<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
    record_iterables: &FiniteRecordBindings,
    record_maps: &FiniteRecordMaps,
) -> Option<FiniteRecord> {
    let mut record = FiniteRecord::default();
    for property in &object.properties {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            return None;
        };
        if property.kind != PropertyKind::Init || property.method || property.computed {
            return None;
        }
        let Some(key) = property.key.static_name() else {
            return None;
        };
        if let Some(values) = finite_strings_from_expression(
            &property.value,
            constants,
            object_maps,
            enum_member_domains,
        ) {
            record.strings.insert(key.to_string(), values);
        }
        if let Some(values) = finite_record_iterable_from_expression(
            &property.value,
            constants,
            object_maps,
            enum_member_domains,
            record_iterables,
            record_maps,
        ) {
            record.record_iterables.insert(key.to_string(), values);
        }
    }
    if record.strings.is_empty() && record.record_iterables.is_empty() {
        None
    } else {
        Some(record)
    }
}

fn finite_records_from_array_elements<'a>(
    elements: impl Iterator<Item = &'a ArrayExpressionElement<'a>>,
    constants: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
    record_iterables: &FiniteRecordBindings,
    record_maps: &FiniteRecordMaps,
) -> Option<FiniteRecords> {
    let mut records = Vec::new();
    for element in elements {
        match element {
            ArrayExpressionElement::ObjectExpression(object) => {
                records.push(finite_record_from_object(
                    object,
                    constants,
                    object_maps,
                    enum_member_domains,
                    record_iterables,
                    record_maps,
                )?)
            }
            ArrayExpressionElement::SpreadElement(spread) => {
                records.extend(finite_record_iterable_from_expression(
                    &spread.argument,
                    constants,
                    object_maps,
                    enum_member_domains,
                    record_iterables,
                    record_maps,
                )?);
            }
            ArrayExpressionElement::Identifier(identifier) => {
                records.extend(record_iterables.get(identifier.name.as_str()).cloned()?);
            }
            _ => return None,
        }
        if records.len() > MAX_FINITE_STRINGS {
            return None;
        }
    }
    Some(records).filter(|records| !records.is_empty())
}

fn finite_record_iterable_from_expression(
    expression: &Expression<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
    record_iterables: &FiniteRecordBindings,
    record_maps: &FiniteRecordMaps,
) -> Option<FiniteRecords> {
    match expression.get_inner_expression() {
        Expression::ArrayExpression(array) => finite_records_from_array_elements(
            array.elements.iter(),
            constants,
            object_maps,
            enum_member_domains,
            record_iterables,
            record_maps,
        ),
        Expression::ObjectExpression(object) => finite_records_from_object_values(
            object,
            constants,
            object_maps,
            enum_member_domains,
            record_iterables,
            record_maps,
        ),
        Expression::Identifier(identifier) => {
            record_iterables.get(identifier.name.as_str()).cloned()
        }
        Expression::StaticMemberExpression(member) => {
            let object = member.object.get_identifier_reference()?;
            let records = record_iterables.get(object.name.as_str())?;
            finite_record_property_iterable(records, member.property.name.as_str())
        }
        Expression::CallExpression(call) => {
            if call_is_callback_value_wrapper(call, "useMemo") {
                return call.arguments.first().and_then(|callback| {
                    finite_records_from_callback_argument(
                        callback,
                        constants,
                        &BTreeMap::new(),
                        object_maps,
                        enum_member_domains,
                        record_iterables,
                        record_maps,
                    )
                });
            }
            let member = call.callee.get_member_expr()?;
            let method = member.static_property_name()?;
            let records = finite_record_iterable_from_expression(
                member.object(),
                constants,
                object_maps,
                enum_member_domains,
                record_iterables,
                record_maps,
            )?;
            match method.as_ref() {
                "filter" => Some(records),
                "slice" => {
                    let start = call.arguments.first().and_then(argument_usize).unwrap_or(0);
                    let end = call
                        .arguments
                        .get(1)
                        .and_then(argument_usize)
                        .unwrap_or(records.len());
                    Some(
                        records
                            .into_iter()
                            .skip(start)
                            .take(end.saturating_sub(start))
                            .collect(),
                    )
                }
                _ => None,
            }
        }
        Expression::TSAsExpression(expression) => finite_record_iterable_from_expression(
            &expression.expression,
            constants,
            object_maps,
            enum_member_domains,
            record_iterables,
            record_maps,
        ),
        Expression::TSSatisfiesExpression(expression) => finite_record_iterable_from_expression(
            &expression.expression,
            constants,
            object_maps,
            enum_member_domains,
            record_iterables,
            record_maps,
        ),
        Expression::TSNonNullExpression(expression) => finite_record_iterable_from_expression(
            &expression.expression,
            constants,
            object_maps,
            enum_member_domains,
            record_iterables,
            record_maps,
        ),
        Expression::TSInstantiationExpression(expression) => {
            finite_record_iterable_from_expression(
                &expression.expression,
                constants,
                object_maps,
                enum_member_domains,
                record_iterables,
                record_maps,
            )
        }
        Expression::TSTypeAssertion(expression) => finite_record_iterable_from_expression(
            &expression.expression,
            constants,
            object_maps,
            enum_member_domains,
            record_iterables,
            record_maps,
        ),
        _ => None,
    }
}

fn finite_record_map_from_expression(
    expression: &Expression<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
    record_iterables: &FiniteRecordBindings,
    record_maps: &FiniteRecordMaps,
) -> Option<FiniteRecords> {
    let Expression::NewExpression(new_expression) = expression.get_inner_expression() else {
        return None;
    };
    if new_expression
        .callee
        .get_identifier_reference()?
        .name
        .as_str()
        != "Map"
    {
        return None;
    }
    let first = new_expression.arguments.first()?;
    let Argument::CallExpression(call) = first else {
        return None;
    };
    let member = call.callee.get_member_expr()?;
    if member.static_property_name()? != "map" {
        return None;
    }
    finite_record_iterable_from_expression(
        member.object(),
        constants,
        object_maps,
        enum_member_domains,
        record_iterables,
        record_maps,
    )
}

fn finite_record_from_expression(
    expression: &Expression<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
    record_iterables: &FiniteRecordBindings,
    record_maps: &FiniteRecordMaps,
) -> Option<FiniteRecords> {
    match expression.get_inner_expression() {
        Expression::ObjectExpression(object) => Some(vec![finite_record_from_object(
            object,
            constants,
            object_maps,
            enum_member_domains,
            record_iterables,
            record_maps,
        )?]),
        Expression::Identifier(identifier) => {
            record_iterables.get(identifier.name.as_str()).cloned()
        }
        Expression::ComputedMemberExpression(member) => {
            let object = member.object.get_identifier_reference()?;
            record_iterables.get(object.name.as_str()).cloned()
        }
        Expression::CallExpression(call) => {
            let member = call.callee.get_member_expr()?;
            if member.static_property_name()? != "get" {
                return None;
            }
            let object = member.object().get_identifier_reference()?;
            record_maps.get(object.name.as_str()).cloned()
        }
        Expression::LogicalExpression(logical) => {
            if logical.operator == LogicalOperator::Coalesce {
                let left = finite_record_from_expression(
                    &logical.left,
                    constants,
                    object_maps,
                    enum_member_domains,
                    record_iterables,
                    record_maps,
                );
                let right = finite_record_from_expression(
                    &logical.right,
                    constants,
                    object_maps,
                    enum_member_domains,
                    record_iterables,
                    record_maps,
                );
                match (left, right) {
                    (Some(left), Some(right)) => merge_finite_records(left, right),
                    (Some(values), None) | (None, Some(values)) => Some(values),
                    (None, None) => None,
                }
            } else {
                None
            }
        }
        Expression::ConditionalExpression(conditional) => {
            let consequent = finite_record_from_expression(
                &conditional.consequent,
                constants,
                object_maps,
                enum_member_domains,
                record_iterables,
                record_maps,
            );
            let alternate = finite_record_from_expression(
                &conditional.alternate,
                constants,
                object_maps,
                enum_member_domains,
                record_iterables,
                record_maps,
            );
            match (consequent, alternate) {
                (Some(consequent), Some(alternate)) => merge_finite_records(consequent, alternate),
                (Some(values), None) | (None, Some(values)) => Some(values),
                (None, None) => None,
            }
        }
        Expression::TSNonNullExpression(expression) => finite_record_from_expression(
            &expression.expression,
            constants,
            object_maps,
            enum_member_domains,
            record_iterables,
            record_maps,
        ),
        _ => None,
    }
}

fn finite_strings_from_array_element(
    element: &ArrayExpressionElement<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    iterables: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
) -> Option<FiniteStrings> {
    match element {
        ArrayExpressionElement::StringLiteral(literal) => {
            Some(finite_string(literal.value.to_string()))
        }
        ArrayExpressionElement::TemplateLiteral(literal) => {
            finite_strings_from_template(literal, constants, object_maps, enum_member_domains)
        }
        ArrayExpressionElement::Identifier(identifier) => {
            constants.get(identifier.name.as_str()).cloned()
        }
        ArrayExpressionElement::CallExpression(call) => {
            finite_strings_from_call(call, constants, object_maps, enum_member_domains)
        }
        ArrayExpressionElement::ComputedMemberExpression(member) => {
            finite_strings_from_computed_member(member, constants, object_maps, enum_member_domains)
        }
        ArrayExpressionElement::StaticMemberExpression(member) => {
            finite_strings_from_static_member(member, object_maps, enum_member_domains)
        }
        ArrayExpressionElement::ConditionalExpression(conditional) => {
            finite_strings_from_conditional(
                conditional,
                constants,
                object_maps,
                enum_member_domains,
            )
        }
        ArrayExpressionElement::ParenthesizedExpression(parenthesized) => {
            finite_strings_from_expression(
                &parenthesized.expression,
                constants,
                object_maps,
                enum_member_domains,
            )
        }
        ArrayExpressionElement::TSAsExpression(expression) => finite_iterable_from_expression(
            &expression.expression,
            constants,
            iterables,
            object_maps,
            enum_member_domains,
        )
        .or_else(|| {
            finite_strings_from_expression(
                &expression.expression,
                constants,
                object_maps,
                enum_member_domains,
            )
        }),
        ArrayExpressionElement::TSSatisfiesExpression(expression) => {
            finite_iterable_from_expression(
                &expression.expression,
                constants,
                iterables,
                object_maps,
                enum_member_domains,
            )
            .or_else(|| {
                finite_strings_from_expression(
                    &expression.expression,
                    constants,
                    object_maps,
                    enum_member_domains,
                )
            })
        }
        ArrayExpressionElement::TSNonNullExpression(expression) => finite_iterable_from_expression(
            &expression.expression,
            constants,
            iterables,
            object_maps,
            enum_member_domains,
        )
        .or_else(|| {
            finite_strings_from_expression(
                &expression.expression,
                constants,
                object_maps,
                enum_member_domains,
            )
        }),
        ArrayExpressionElement::TSInstantiationExpression(expression) => {
            finite_iterable_from_expression(
                &expression.expression,
                constants,
                iterables,
                object_maps,
                enum_member_domains,
            )
            .or_else(|| {
                finite_strings_from_expression(
                    &expression.expression,
                    constants,
                    object_maps,
                    enum_member_domains,
                )
            })
        }
        ArrayExpressionElement::TSTypeAssertion(expression) => finite_iterable_from_expression(
            &expression.expression,
            constants,
            iterables,
            object_maps,
            enum_member_domains,
        )
        .or_else(|| {
            finite_strings_from_expression(
                &expression.expression,
                constants,
                object_maps,
                enum_member_domains,
            )
        }),
        _ => None,
    }
}

fn finite_strings_from_call(
    call: &CallExpression<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
) -> Option<FiniteStrings> {
    if !call.arguments.is_empty() {
        return None;
    }

    let member = call.callee.get_member_expr()?;
    let method = member.static_property_name()?;
    let values = finite_strings_from_expression(
        member.object(),
        constants,
        object_maps,
        enum_member_domains,
    )?;

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
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
) -> Option<FiniteStrings> {
    let consequent = finite_strings_from_expression(
        &conditional.consequent,
        constants,
        object_maps,
        enum_member_domains,
    )?;
    let alternate = finite_strings_from_expression(
        &conditional.alternate,
        constants,
        object_maps,
        enum_member_domains,
    )?;
    union_finite_strings(consequent, alternate)
}

fn finite_strings_from_template(
    literal: &TemplateLiteral<'_>,
    constants: &BTreeMap<String, FiniteStrings>,
    object_maps: &FiniteObjectMaps,
    enum_member_domains: &TypeDomains,
) -> Option<FiniteStrings> {
    let mut values = finite_string("");
    for (index, quasi) in literal.quasis.iter().enumerate() {
        let cooked = quasi.value.cooked.as_ref()?;
        values = append_finite_strings(values, &finite_string(cooked.to_string()))?;

        if let Some(expression) = literal.expressions.get(index) {
            let expression_values = finite_strings_from_expression(
                expression,
                constants,
                object_maps,
                enum_member_domains,
            )?;
            values = append_finite_strings(values, &expression_values)?;
        }
    }
    Some(values)
}

struct HelperBodyCollector {
    param_names: BTreeMap<String, usize>,
    helpers: BTreeMap<String, HelperSummary>,
    finite_constants: BTreeMap<String, FiniteStrings>,
    finite_iterables: BTreeMap<String, FiniteStrings>,
    finite_object_maps: FiniteObjectMaps,
    finite_record_constants: FiniteRecordBindings,
    finite_record_iterables: FiniteRecordBindings,
    finite_record_maps: FiniteRecordMaps,
    enum_member_domains: TypeDomains,
    usages: Vec<HelperParamUsage>,
}

struct MessageKeyHelperCollector {
    param_names: BTreeMap<String, usize>,
    param_indexes: BTreeSet<usize>,
}

impl<'a> Visit<'a> for MessageKeyHelperCollector {
    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        let Some(callee) = call
            .callee
            .get_inner_expression()
            .get_identifier_reference()
            .map(|identifier| identifier.name.as_str())
        else {
            walk::walk_call_expression(self, call);
            return;
        };
        if callee == "getMessage" {
            if let Some(Argument::Identifier(identifier)) = call.arguments.get(1) {
                if let Some(index) = self.param_names.get(identifier.name.as_str()) {
                    self.param_indexes.insert(*index);
                }
            }
        }
        walk::walk_call_expression(self, call);
    }

    fn visit_function(&mut self, _function: &Function<'a>, _flags: ScopeFlags) {}

    fn visit_arrow_function_expression(&mut self, _arrow: &ArrowFunctionExpression<'a>) {}
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

    fn helper_summary_for_callee(&self, expression: &Expression<'_>) -> Option<HelperSummary> {
        let callee = expression
            .get_inner_expression()
            .get_identifier_reference()
            .map(|identifier| identifier.name.as_str())?;
        self.helpers.get(callee).cloned()
    }

    fn param_index_from_argument(&self, argument: &Argument<'_>) -> Option<usize> {
        let Argument::Identifier(identifier) = argument else {
            return None;
        };
        self.param_names.get(identifier.name.as_str()).copied()
    }

    fn apply_helper_summary(&mut self, call: &CallExpression<'_>, summary: &HelperSummary) {
        for usage in &summary.param_usages {
            let Some(argument) = call.arguments.get(usage.param_index) else {
                continue;
            };
            let Some(param_index) = self.param_index_from_argument(argument) else {
                continue;
            };
            self.usages.push(HelperParamUsage {
                param_index,
                keys: usage.keys.clone(),
            });
        }
    }

    fn finite_iterable_from_expression(
        &self,
        expression: &Expression<'_>,
    ) -> Option<FiniteStrings> {
        finite_iterable_from_expression(
            expression,
            &self.finite_constants,
            &self.finite_iterables,
            &self.finite_object_maps,
            &self.enum_member_domains,
        )
    }

    fn finite_record_iterable_from_expression(
        &self,
        expression: &Expression<'_>,
    ) -> Option<FiniteRecords> {
        match expression.get_inner_expression() {
            Expression::StaticMemberExpression(member) => {
                let object = member.object.get_identifier_reference()?;
                let records = self.finite_record_constants.get(object.name.as_str())?;
                finite_record_property_iterable(records, member.property.name.as_str())
            }
            _ => finite_record_iterable_from_expression(
                expression,
                &self.finite_constants,
                &self.finite_object_maps,
                &self.enum_member_domains,
                &self.finite_record_iterables,
                &self.finite_record_maps,
            ),
        }
    }

    fn finite_record_from_expression(&self, expression: &Expression<'_>) -> Option<FiniteRecords> {
        match expression.get_inner_expression() {
            Expression::LogicalExpression(logical)
                if logical.operator == LogicalOperator::Coalesce =>
            {
                let left = self.finite_record_from_expression(&logical.left);
                let right = self.finite_record_from_expression(&logical.right);
                return match (left, right) {
                    (Some(left), Some(right)) => merge_finite_records(left, right),
                    (Some(values), None) | (None, Some(values)) => Some(values),
                    (None, None) => None,
                };
            }
            Expression::ConditionalExpression(conditional) => {
                let consequent = self.finite_record_from_expression(&conditional.consequent);
                let alternate = self.finite_record_from_expression(&conditional.alternate);
                return match (consequent, alternate) {
                    (Some(consequent), Some(alternate)) => {
                        merge_finite_records(consequent, alternate)
                    }
                    (Some(values), None) | (None, Some(values)) => Some(values),
                    (None, None) => None,
                };
            }
            _ => {}
        }
        finite_record_from_expression(
            expression,
            &self.finite_constants,
            &self.finite_object_maps,
            &self.enum_member_domains,
            &self.finite_record_iterables,
            &self.finite_record_maps,
        )
        .or_else(|| {
            if let Expression::Identifier(identifier) = expression.get_inner_expression() {
                self.finite_record_constants
                    .get(identifier.name.as_str())
                    .cloned()
            } else {
                None
            }
        })
    }

    fn finite_strings_from_argument(&self, argument: &Argument<'_>) -> Option<FiniteStrings> {
        match argument {
            Argument::StaticMemberExpression(member) => {
                let object = member.object.get_identifier_reference()?;
                let records = self.finite_record_constants.get(object.name.as_str())?;
                finite_record_property_strings(records, member.property.name.as_str())
            }
            _ => finite_strings_from_argument(
                argument,
                &self.finite_constants,
                &self.finite_object_maps,
                &self.enum_member_domains,
            ),
        }
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

    fn with_finite_record_constant(
        &mut self,
        name: &str,
        values: FiniteRecords,
        visit: impl FnOnce(&mut Self),
    ) {
        let previous = self
            .finite_record_constants
            .insert(name.to_string(), values);
        visit(self);
        match previous {
            Some(values) => {
                self.finite_record_constants
                    .insert(name.to_string(), values);
            }
            None => {
                self.finite_record_constants.remove(name);
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

    fn visit_finite_record_iteration_callback(
        &mut self,
        callback: &Argument<'_>,
        values: FiniteRecords,
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
                self.with_finite_record_constant(param, values, |collector| {
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
                self.with_finite_record_constant(param, values, |collector| {
                    walk::walk_function(collector, function, ScopeFlags::Function);
                });
                true
            }
            _ => false,
        }
    }

    fn visit_finite_record_iteration_call(&mut self, call: &CallExpression<'_>) -> bool {
        let Some(member) = call.callee.get_member_expr() else {
            return false;
        };
        let Some(method) = member.static_property_name() else {
            return false;
        };
        if !matches!(method.as_ref(), "map" | "forEach") {
            return false;
        }
        let Some(values) = self.finite_record_iterable_from_expression(member.object()) else {
            return false;
        };
        let Some(callback) = call.arguments.first() else {
            return false;
        };
        self.visit_finite_record_iteration_callback(callback, values)
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
                    record_enum_member_domains_from_expression(init, &mut self.enum_member_domains);
                    if let Some(values) = finite_strings_from_expression(
                        init,
                        &self.finite_constants,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                    ) {
                        self.finite_constants.insert(name.to_string(), values);
                    }
                    if let Some(values) = self.finite_iterable_from_expression(init) {
                        self.finite_iterables.insert(name.to_string(), values);
                    }
                    if let Some(values) = finite_object_map_from_expression(
                        init,
                        &self.finite_constants,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                    ) {
                        self.finite_object_maps.insert(name.to_string(), values);
                    }
                    if let Some(values) = self.finite_record_iterable_from_expression(init) {
                        self.finite_record_iterables
                            .insert(name.to_string(), values);
                    }
                    if let Some(values) = self.finite_record_from_expression(init) {
                        self.finite_record_constants
                            .insert(name.to_string(), values);
                    }
                    if let Some(values) = finite_record_map_from_expression(
                        init,
                        &self.finite_constants,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                        &self.finite_record_iterables,
                        &self.finite_record_maps,
                    ) {
                        self.finite_record_maps.insert(name.to_string(), values);
                    }
                }
            }
        }
        walk::walk_variable_declarator(self, declarator);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if self.visit_finite_record_iteration_call(call) {
            if let Some(member) = call.callee.get_member_expr() {
                self.visit_expression(member.object());
            }
            return;
        }

        if self.visit_finite_iteration_call(call) {
            if let Some(member) = call.callee.get_member_expr() {
                self.visit_expression(member.object());
            }
            return;
        }

        if let Some(summary) = self.helper_summary_for_callee(&call.callee) {
            self.apply_helper_summary(call, &summary);
        }

        if let Some(param_index) = self.callee_param_index(&call.callee) {
            let keys = call
                .arguments
                .first()
                .and_then(|argument| self.finite_strings_from_argument(argument));
            self.usages.push(HelperParamUsage { param_index, keys });
        }
        walk::walk_call_expression(self, call);
    }

    fn visit_function(&mut self, _function: &Function<'a>, _flags: ScopeFlags) {}

    fn visit_arrow_function_expression(&mut self, _arrow: &ArrowFunctionExpression<'a>) {}

    fn visit_for_of_statement(&mut self, statement: &ForOfStatement<'a>) {
        if let Some(values) = self.finite_record_iterable_from_expression(&statement.right) {
            if let Some(binding) = self.for_of_binding_name(statement).map(str::to_string) {
                self.with_finite_record_constant(&binding, values, |collector| {
                    collector.visit_statement(&statement.body);
                });
                return;
            }
        }

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
    finite_object_maps: FiniteObjectMaps,
    enum_member_domains: TypeDomains,
    return_values: FiniteStrings,
    unknown_return: bool,
}

struct ReturnRecordCollector {
    finite_constants: BTreeMap<String, FiniteStrings>,
    finite_iterables: BTreeMap<String, FiniteStrings>,
    finite_object_maps: FiniteObjectMaps,
    finite_record_constants: FiniteRecordBindings,
    finite_record_iterables: FiniteRecordBindings,
    finite_record_maps: FiniteRecordMaps,
    enum_member_domains: TypeDomains,
    return_records: FiniteRecords,
    unknown_return: bool,
}

impl ReturnRecordCollector {
    fn finite_record_iterable_from_expression(
        &self,
        expression: &Expression<'_>,
    ) -> Option<FiniteRecords> {
        finite_record_iterable_from_expression(
            expression,
            &self.finite_constants,
            &self.finite_object_maps,
            &self.enum_member_domains,
            &self.finite_record_iterables,
            &self.finite_record_maps,
        )
    }

    fn finite_record_from_expression(&self, expression: &Expression<'_>) -> Option<FiniteRecords> {
        finite_record_from_expression(
            expression,
            &self.finite_constants,
            &self.finite_object_maps,
            &self.enum_member_domains,
            &self.finite_record_iterables,
            &self.finite_record_maps,
        )
        .or_else(|| {
            if let Expression::Identifier(identifier) = expression.get_inner_expression() {
                self.finite_record_constants
                    .get(identifier.name.as_str())
                    .cloned()
            } else {
                None
            }
        })
    }

    fn record_const_init(&mut self, name: &str, init: &Expression<'_>) {
        record_enum_member_domains_from_expression(init, &mut self.enum_member_domains);
        if let Some(values) = finite_strings_from_expression(
            init,
            &self.finite_constants,
            &self.finite_object_maps,
            &self.enum_member_domains,
        ) {
            self.finite_constants.insert(name.to_string(), values);
        }
        if let Some(values) = finite_iterable_from_expression(
            init,
            &self.finite_constants,
            &self.finite_iterables,
            &self.finite_object_maps,
            &self.enum_member_domains,
        ) {
            self.finite_iterables.insert(name.to_string(), values);
        }
        if let Some(values) = finite_object_map_from_expression(
            init,
            &self.finite_constants,
            &self.finite_object_maps,
            &self.enum_member_domains,
        ) {
            self.finite_object_maps.insert(name.to_string(), values);
        }
        if let Some(values) = self.finite_record_iterable_from_expression(init) {
            self.finite_record_iterables
                .insert(name.to_string(), values);
        }
        if let Some(values) = finite_record_map_from_expression(
            init,
            &self.finite_constants,
            &self.finite_object_maps,
            &self.enum_member_domains,
            &self.finite_record_iterables,
            &self.finite_record_maps,
        ) {
            self.finite_record_maps.insert(name.to_string(), values);
        }
    }
}

impl<'a> Visit<'a> for ReturnRecordCollector {
    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        if declarator.kind == VariableDeclarationKind::Const {
            if let Some(name) = binding_identifier_name(&declarator.id) {
                if let Some(init) = &declarator.init {
                    self.record_const_init(name, init);
                }
            }
        }
        walk::walk_variable_declarator(self, declarator);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        let Some(member) = call.callee.get_member_expr() else {
            walk::walk_call_expression(self, call);
            return;
        };
        if member
            .static_property_name()
            .is_some_and(|method| method == "push")
        {
            if let Some(object) = member.object().get_identifier_reference() {
                let mut appended = Vec::new();
                for argument in &call.arguments {
                    let Argument::ObjectExpression(object) = argument else {
                        continue;
                    };
                    if let Some(record) = finite_record_from_object(
                        object,
                        &self.finite_constants,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                        &self.finite_record_iterables,
                        &self.finite_record_maps,
                    ) {
                        appended.push(record);
                    }
                }
                if !appended.is_empty() {
                    self.finite_record_iterables
                        .entry(object.name.to_string())
                        .or_default()
                        .extend(appended);
                    return;
                }
            }
        }
        walk::walk_call_expression(self, call);
    }

    fn visit_return_statement(&mut self, statement: &ReturnStatement<'a>) {
        let Some(argument) = &statement.argument else {
            self.unknown_return = true;
            return;
        };
        let Some(records) = self
            .finite_record_from_expression(argument)
            .or_else(|| self.finite_record_iterable_from_expression(argument))
        else {
            self.unknown_return = true;
            return;
        };
        if let Some(records) = merge_finite_records(self.return_records.clone(), records) {
            self.return_records = records;
        } else {
            self.unknown_return = true;
        }
    }

    fn visit_function(&mut self, _function: &Function<'a>, _flags: ScopeFlags) {}

    fn visit_arrow_function_expression(&mut self, _arrow: &ArrowFunctionExpression<'a>) {}
}

impl ReturnValueCollector {
    fn finite_iterable_from_expression(
        &self,
        expression: &Expression<'_>,
    ) -> Option<FiniteStrings> {
        finite_iterable_from_expression(
            expression,
            &self.finite_constants,
            &self.finite_iterables,
            &self.finite_object_maps,
            &self.enum_member_domains,
        )
    }
}

impl<'a> Visit<'a> for ReturnValueCollector {
    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        if declarator.kind == VariableDeclarationKind::Const {
            if let Some(name) = binding_identifier_name(&declarator.id) {
                if let Some(init) = &declarator.init {
                    record_enum_member_domains_from_expression(init, &mut self.enum_member_domains);
                    if let Some(values) = finite_strings_from_expression(
                        init,
                        &self.finite_constants,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                    ) {
                        self.finite_constants.insert(name.to_string(), values);
                    }
                    if let Some(values) = self.finite_iterable_from_expression(init) {
                        self.finite_iterables.insert(name.to_string(), values);
                    }
                    if let Some(values) = finite_object_map_from_expression(
                        init,
                        &self.finite_constants,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                    ) {
                        self.finite_object_maps.insert(name.to_string(), values);
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
        let Some(values) = finite_strings_from_expression(
            argument,
            &self.finite_constants,
            &self.finite_object_maps,
            &self.enum_member_domains,
        ) else {
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
    finite_constants: BTreeMap<String, FiniteStrings>,
    finite_iterables: BTreeMap<String, FiniteStrings>,
    finite_object_maps: FiniteObjectMaps,
    finite_record_constants: FiniteRecordBindings,
    finite_record_iterables: FiniteRecordBindings,
    finite_record_maps: FiniteRecordMaps,
    return_record_helpers: BTreeMap<String, FiniteRecords>,
    enum_member_domains: TypeDomains,
    export_locals: BTreeMap<String, String>,
    default_local: Option<String>,
    default_summary: Option<HelperSummary>,
    default_return_summary: Option<FiniteStrings>,
    default_iterable_summary: Option<FiniteStrings>,
    default_record_iterable_export: Option<FiniteRecords>,
    default_record_return_summary: Option<FiniteRecords>,
}

impl SourceIndexCollector {
    fn finish(self) -> SourceFileIndex {
        let mut named_exports = BTreeMap::new();
        let mut named_return_exports = BTreeMap::new();
        let mut named_iterable_exports = BTreeMap::new();
        let mut named_record_iterable_exports = BTreeMap::new();
        let mut named_record_return_exports = BTreeMap::new();
        for (exported, local) in self.export_locals {
            if let Some(summary) = self.helpers.get(&local) {
                named_exports.insert(exported.clone(), summary.clone());
            }
            if let Some(summary) = self.return_helpers.get(&local) {
                named_return_exports.insert(exported.clone(), summary.clone());
            }
            if let Some(values) = self.finite_iterables.get(&local) {
                named_iterable_exports.insert(exported.clone(), values.clone());
            }
            if let Some(values) = self.finite_record_iterables.get(&local) {
                named_record_iterable_exports.insert(exported.clone(), values.clone());
            }
            if let Some(values) = self.return_record_helpers.get(&local) {
                named_record_return_exports.insert(exported, values.clone());
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
        let default_iterable_export = self.default_iterable_summary.or_else(|| {
            default_local
                .as_ref()
                .and_then(|name| self.finite_iterables.get(name).cloned())
        });
        let default_record_iterable_export = self.default_record_iterable_export.or_else(|| {
            default_local
                .as_ref()
                .and_then(|name| self.finite_record_iterables.get(name).cloned())
        });
        let default_record_return_export = self.default_record_return_summary.or_else(|| {
            default_local
                .as_ref()
                .and_then(|name| self.return_record_helpers.get(name).cloned())
        });

        SourceFileIndex {
            imports: self.imports,
            helpers: self.helpers,
            return_helpers: self.return_helpers,
            return_record_helpers: self.return_record_helpers,
            named_exports,
            named_return_exports,
            named_iterable_exports,
            named_record_iterable_exports,
            named_record_return_exports,
            default_export,
            default_return_export,
            default_iterable_export,
            default_record_iterable_export,
            default_record_return_export,
        }
    }

    fn record_helper(&mut self, name: &str, summary: HelperSummary) {
        self.helpers.insert(name.to_string(), summary);
    }

    fn record_return_helper(&mut self, name: &str, summary: FiniteStrings) {
        self.return_helpers.insert(name.to_string(), summary);
    }

    fn record_return_record_helper(&mut self, name: &str, summary: FiniteRecords) {
        self.return_record_helpers.insert(name.to_string(), summary);
    }

    fn record_finite_constant(&mut self, name: &str, values: FiniteStrings) {
        self.finite_constants.insert(name.to_string(), values);
    }

    fn record_finite_iterable(&mut self, name: &str, values: FiniteStrings) {
        self.finite_iterables.insert(name.to_string(), values);
    }

    fn record_finite_record_iterable(&mut self, name: &str, values: FiniteRecords) {
        self.finite_record_iterables
            .insert(name.to_string(), values);
    }

    fn record_finite_record_constant(&mut self, name: &str, values: FiniteRecords) {
        self.finite_record_constants
            .insert(name.to_string(), values);
    }

    fn record_variable_helpers(&mut self, declaration: &VariableDeclaration<'_>) {
        for declarator in &declaration.declarations {
            let Some(name) = binding_identifier_name(&declarator.id) else {
                continue;
            };
            let Some(init) = &declarator.init else {
                continue;
            };
            if let Some(summary) = helper_summary_from_expression_with_context(
                init,
                &self.helpers,
                &self.finite_constants,
                &self.finite_iterables,
                &self.finite_object_maps,
                &self.finite_record_constants,
                &self.finite_record_iterables,
                &self.finite_record_maps,
                &self.enum_member_domains,
            ) {
                self.record_helper(name, summary);
            }
            if let Some(summary) = finite_return_summary_from_expression(init) {
                self.record_return_helper(name, summary);
            }
            if let Some(summary) = finite_record_return_summary_from_expression(init) {
                self.record_return_record_helper(name, summary);
            }
        }
    }

    fn record_variable_finite_values(&mut self, declaration: &VariableDeclaration<'_>) {
        if declaration.kind != VariableDeclarationKind::Const {
            return;
        }
        for declarator in &declaration.declarations {
            let Some(name) = binding_identifier_name(&declarator.id) else {
                continue;
            };
            let Some(init) = &declarator.init else {
                continue;
            };
            record_enum_member_domains_from_expression(init, &mut self.enum_member_domains);
            if let Some(values) = finite_strings_from_expression(
                init,
                &self.finite_constants,
                &self.finite_object_maps,
                &self.enum_member_domains,
            ) {
                self.record_finite_constant(name, values);
            }
            if let Some(values) = finite_iterable_from_expression(
                init,
                &self.finite_constants,
                &self.finite_iterables,
                &self.finite_object_maps,
                &self.enum_member_domains,
            ) {
                self.record_finite_iterable(name, values);
            }
            if let Some(values) = finite_object_map_from_expression(
                init,
                &self.finite_constants,
                &self.finite_object_maps,
                &self.enum_member_domains,
            ) {
                self.finite_object_maps.insert(name.to_string(), values);
            }
            if let Some(values) = finite_record_iterable_from_expression(
                init,
                &self.finite_constants,
                &self.finite_object_maps,
                &self.enum_member_domains,
                &self.finite_record_iterables,
                &self.finite_record_maps,
            ) {
                self.record_finite_record_iterable(name, values);
            }
            if let Some(values) = finite_record_from_expression(
                init,
                &self.finite_constants,
                &self.finite_object_maps,
                &self.enum_member_domains,
                &self.finite_record_iterables,
                &self.finite_record_maps,
            ) {
                self.record_finite_record_constant(name, values);
            }
            if let Some(values) = finite_record_map_from_expression(
                init,
                &self.finite_constants,
                &self.finite_object_maps,
                &self.enum_member_domains,
                &self.finite_record_iterables,
                &self.finite_record_maps,
            ) {
                self.finite_record_maps.insert(name.to_string(), values);
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
                if let Some(summary) = helper_summary_from_function_with_context(
                    function,
                    &self.helpers,
                    &self.finite_constants,
                    &self.finite_iterables,
                    &self.finite_object_maps,
                    &self.finite_record_constants,
                    &self.finite_record_iterables,
                    &self.finite_record_maps,
                    &self.enum_member_domains,
                ) {
                    self.default_summary = Some(summary);
                }
                if let Some(summary) = finite_return_summary_from_function(function) {
                    self.default_return_summary = Some(summary);
                }
                let default_record_return_summary =
                    finite_record_return_summary_from_function_with_context(
                        function,
                        &self.finite_constants,
                        &self.finite_iterables,
                        &self.finite_object_maps,
                        &self.finite_record_constants,
                        &self.finite_record_iterables,
                        &self.finite_record_maps,
                        &self.enum_member_domains,
                    );
                self.default_record_return_summary = default_record_return_summary.clone();
                if let Some(id) = &function.id {
                    self.record_helper(
                        id.name.as_str(),
                        self.default_summary.clone().unwrap_or_default(),
                    );
                    if let Some(summary) = self.default_return_summary.clone() {
                        self.record_return_helper(id.name.as_str(), summary);
                    }
                    if let Some(summary) = default_record_return_summary {
                        self.record_return_record_helper(id.name.as_str(), summary);
                    }
                }
            }
            ExportDefaultDeclarationKind::Identifier(identifier) => {
                self.default_local = Some(identifier.name.to_string());
            }
            ExportDefaultDeclarationKind::ArrowFunctionExpression(arrow) => {
                self.default_summary = Some(helper_summary_from_arrow_with_context(
                    arrow,
                    &self.helpers,
                    &self.finite_constants,
                    &self.finite_iterables,
                    &self.finite_object_maps,
                    &self.finite_record_constants,
                    &self.finite_record_iterables,
                    &self.finite_record_maps,
                    &self.enum_member_domains,
                ));
                self.default_return_summary = finite_return_summary_from_arrow(arrow);
                if let Some(summary) = finite_record_return_summary_from_arrow(arrow) {
                    self.default_record_return_summary = Some(summary);
                }
            }
            ExportDefaultDeclarationKind::FunctionExpression(function) => {
                self.default_summary = helper_summary_from_function_with_context(
                    function,
                    &self.helpers,
                    &self.finite_constants,
                    &self.finite_iterables,
                    &self.finite_object_maps,
                    &self.finite_record_constants,
                    &self.finite_record_iterables,
                    &self.finite_record_maps,
                    &self.enum_member_domains,
                );
                self.default_return_summary = finite_return_summary_from_function(function);
                if let Some(summary) = finite_record_return_summary_from_function(function) {
                    self.default_record_return_summary = Some(summary);
                }
            }
            ExportDefaultDeclarationKind::ArrayExpression(array) => {
                self.default_iterable_summary = finite_iterable_from_array_elements(
                    array.elements.iter(),
                    &self.finite_constants,
                    &self.finite_iterables,
                    &self.finite_object_maps,
                    &self.enum_member_domains,
                );
                self.default_record_iterable_export = finite_records_from_array_elements(
                    array.elements.iter(),
                    &self.finite_constants,
                    &self.finite_object_maps,
                    &self.enum_member_domains,
                    &self.finite_record_iterables,
                    &self.finite_record_maps,
                );
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
            if let Some(summary) = helper_summary_from_function_with_context(
                function,
                &self.helpers,
                &self.finite_constants,
                &self.finite_iterables,
                &self.finite_object_maps,
                &self.finite_record_constants,
                &self.finite_record_iterables,
                &self.finite_record_maps,
                &self.enum_member_domains,
            ) {
                self.record_helper(id.name.as_str(), summary);
            }
            if let Some(summary) = finite_return_summary_from_function(function) {
                self.record_return_helper(id.name.as_str(), summary);
            }
            if let Some(summary) = finite_record_return_summary_from_function_with_context(
                function,
                &self.finite_constants,
                &self.finite_iterables,
                &self.finite_object_maps,
                &self.finite_record_constants,
                &self.finite_record_iterables,
                &self.finite_record_maps,
                &self.enum_member_domains,
            ) {
                self.record_return_record_helper(id.name.as_str(), summary);
            }
        }
        walk::walk_function(self, function, flags);
    }

    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        if let Some(name) = binding_identifier_name(&declarator.id) {
            if let Some(init) = &declarator.init {
                if declarator.kind == VariableDeclarationKind::Const {
                    record_enum_member_domains_from_expression(init, &mut self.enum_member_domains);
                    if let Some(values) = finite_strings_from_expression(
                        init,
                        &self.finite_constants,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                    ) {
                        self.record_finite_constant(name, values);
                    }
                    if let Some(values) = finite_iterable_from_expression(
                        init,
                        &self.finite_constants,
                        &self.finite_iterables,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                    ) {
                        self.record_finite_iterable(name, values);
                    }
                    if let Some(values) = finite_object_map_from_expression(
                        init,
                        &self.finite_constants,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                    ) {
                        self.finite_object_maps.insert(name.to_string(), values);
                    }
                    if let Some(values) = finite_record_iterable_from_expression(
                        init,
                        &self.finite_constants,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                        &self.finite_record_iterables,
                        &self.finite_record_maps,
                    ) {
                        self.record_finite_record_iterable(name, values);
                    }
                    if let Some(values) = finite_record_from_expression(
                        init,
                        &self.finite_constants,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                        &self.finite_record_iterables,
                        &self.finite_record_maps,
                    ) {
                        self.record_finite_record_constant(name, values);
                    }
                    if let Some(values) = finite_record_map_from_expression(
                        init,
                        &self.finite_constants,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                        &self.finite_record_iterables,
                        &self.finite_record_maps,
                    ) {
                        self.finite_record_maps.insert(name.to_string(), values);
                    }
                }
                if let Some(summary) = helper_summary_from_expression_with_context(
                    init,
                    &self.helpers,
                    &self.finite_constants,
                    &self.finite_iterables,
                    &self.finite_object_maps,
                    &self.finite_record_constants,
                    &self.finite_record_iterables,
                    &self.finite_record_maps,
                    &self.enum_member_domains,
                ) {
                    self.record_helper(name, summary);
                }
                if let Some(summary) = finite_return_summary_from_expression(init) {
                    self.record_return_helper(name, summary);
                }
                if let Some(summary) = finite_record_return_summary_from_expression(init) {
                    self.record_return_record_helper(name, summary);
                }
            }
        }
        walk::walk_variable_declarator(self, declarator);
    }

    fn visit_export_named_declaration(&mut self, declaration: &ExportNamedDeclaration<'a>) {
        if let Some(inner) = &declaration.declaration {
            self.record_export_declaration(inner);
            if let Declaration::VariableDeclaration(variable) = inner {
                self.record_variable_finite_values(variable);
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
            finite_object_maps: BTreeMap::new(),
            finite_record_constants: BTreeMap::new(),
            finite_record_iterables: BTreeMap::new(),
            finite_record_maps: BTreeMap::new(),
            typed_object_property_domains: BTreeMap::new(),
            type_domains: BTreeMap::new(),
            type_property_domains: BTreeMap::new(),
            zod_schema_property_domains: BTreeMap::new(),
            enum_member_domains: BTreeMap::new(),
            translators: BTreeMap::new(),
            message_key_helpers: BTreeMap::new(),
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

    fn zod_schema_property_domains_from_expression(
        &self,
        expression: &Expression<'_>,
    ) -> Option<PropertyDomains> {
        match expression.get_inner_expression() {
            Expression::CallExpression(call) => {
                let member = call.callee.get_member_expr()?;
                let method = member.static_property_name()?;
                if method == "object" && is_identifier_expression(member.object(), "z") {
                    let Argument::ObjectExpression(object) = call.arguments.first()? else {
                        return None;
                    };
                    return self.zod_schema_property_domains_from_object(object);
                }

                if matches!(
                    method.as_ref(),
                    "and"
                        | "brand"
                        | "catch"
                        | "default"
                        | "describe"
                        | "nullable"
                        | "nullish"
                        | "optional"
                        | "or"
                        | "pipe"
                        | "readonly"
                        | "refine"
                        | "superRefine"
                        | "transform"
                ) {
                    return self.zod_schema_property_domains_from_expression(member.object());
                }

                None
            }
            Expression::TSAsExpression(expression) => {
                self.zod_schema_property_domains_from_expression(&expression.expression)
            }
            Expression::TSSatisfiesExpression(expression) => {
                self.zod_schema_property_domains_from_expression(&expression.expression)
            }
            Expression::TSNonNullExpression(expression) => {
                self.zod_schema_property_domains_from_expression(&expression.expression)
            }
            Expression::TSTypeAssertion(expression) => {
                self.zod_schema_property_domains_from_expression(&expression.expression)
            }
            _ => None,
        }
    }

    fn zod_schema_property_domains_from_object(
        &self,
        object: &ObjectExpression<'_>,
    ) -> Option<PropertyDomains> {
        let mut properties = BTreeMap::new();
        for property in &object.properties {
            let ObjectPropertyKind::ObjectProperty(property) = property else {
                continue;
            };
            if property.kind != PropertyKind::Init || property.method || property.computed {
                continue;
            }
            let Some(name) = property.key.static_name() else {
                continue;
            };
            if let Some(values) = self.zod_finite_values_from_expression(&property.value) {
                properties.insert(name.to_string(), values);
            }
        }
        Some(properties).filter(|properties| !properties.is_empty())
    }

    fn zod_finite_values_from_expression(
        &self,
        expression: &Expression<'_>,
    ) -> Option<FiniteStrings> {
        match expression.get_inner_expression() {
            Expression::CallExpression(call) => self.zod_finite_values_from_call(call),
            Expression::TSAsExpression(expression) => {
                self.zod_finite_values_from_expression(&expression.expression)
            }
            Expression::TSSatisfiesExpression(expression) => {
                self.zod_finite_values_from_expression(&expression.expression)
            }
            Expression::TSNonNullExpression(expression) => {
                self.zod_finite_values_from_expression(&expression.expression)
            }
            Expression::TSTypeAssertion(expression) => {
                self.zod_finite_values_from_expression(&expression.expression)
            }
            _ => None,
        }
    }

    fn zod_finite_values_from_call(&self, call: &CallExpression<'_>) -> Option<FiniteStrings> {
        let member = call.callee.get_member_expr()?;
        let method = member.static_property_name()?;
        if method == "enum" && is_identifier_expression(member.object(), "z") {
            let first = call.arguments.first()?;
            return self.finite_iterable_from_zod_argument(first);
        }
        if method == "literal" && is_identifier_expression(member.object(), "z") {
            let first = call.arguments.first()?;
            return self.finite_strings_from_argument(first);
        }
        if method == "union" && is_identifier_expression(member.object(), "z") {
            let Argument::ArrayExpression(array) = call.arguments.first()? else {
                return None;
            };
            let mut values = BTreeSet::new();
            for element in &array.elements {
                let ArrayExpressionElement::CallExpression(call) = element else {
                    return None;
                };
                values.extend(self.zod_finite_values_from_call(call)?);
                if values.len() > MAX_FINITE_STRINGS {
                    return None;
                }
            }
            return Some(values);
        }

        if matches!(
            method.as_ref(),
            "and"
                | "brand"
                | "catch"
                | "default"
                | "describe"
                | "nullable"
                | "nullish"
                | "optional"
                | "or"
                | "pipe"
                | "readonly"
                | "refine"
                | "superRefine"
                | "transform"
        ) {
            return self.zod_finite_values_from_expression(member.object());
        }

        None
    }

    fn finite_iterable_from_zod_argument(&self, argument: &Argument<'_>) -> Option<FiniteStrings> {
        match argument {
            Argument::ArrayExpression(array) => finite_iterable_from_array_elements(
                array.elements.iter(),
                &self.finite_constants,
                &self.finite_iterables,
                &self.finite_object_maps,
                &self.enum_member_domains,
            ),
            Argument::Identifier(identifier) => self
                .finite_iterables
                .get(identifier.name.as_str())
                .cloned()
                .or_else(|| self.finite_iterable_for_imported_identifier(identifier.name.as_str())),
            _ => self.finite_strings_from_argument(argument),
        }
    }

    fn record_ts_type_alias(&mut self, declaration: &TSTypeAliasDeclaration<'_>) {
        let name = declaration.id.name.as_str();
        if let Some(values) = finite_strings_from_ts_type(
            &declaration.type_annotation,
            &self.type_domains,
            &self.enum_member_domains,
        ) {
            self.type_domains.insert(name.to_string(), values);
        }
        if let Some(properties) = property_domains_from_ts_type(
            &declaration.type_annotation,
            &self.type_property_domains,
            &self.type_domains,
            &self.enum_member_domains,
            &self.zod_schema_property_domains,
        ) {
            self.type_property_domains
                .insert(name.to_string(), properties);
        }
    }

    fn record_ts_interface(&mut self, declaration: &TSInterfaceDeclaration<'_>) {
        let properties = property_domains_from_type_literal(
            &declaration.body.body,
            &self.type_property_domains,
            &self.type_domains,
            &self.enum_member_domains,
        );
        if !properties.is_empty() {
            self.type_property_domains
                .insert(declaration.id.name.to_string(), properties);
        }
    }

    fn bind_pattern_type_domains(
        &mut self,
        pattern: &BindingPattern<'_>,
        type_annotation: &TSType<'_>,
    ) {
        if let Some(name) = binding_identifier_name(pattern) {
            if let Some(values) = finite_strings_from_ts_type(
                type_annotation,
                &self.type_domains,
                &self.enum_member_domains,
            ) {
                self.finite_constants.insert(name.to_string(), values);
            }
            if let Some(values) = finite_iterable_from_ts_type(
                type_annotation,
                &self.type_domains,
                &self.enum_member_domains,
            ) {
                self.finite_iterables.insert(name.to_string(), values);
            }
            if let Some(properties) = property_domains_from_ts_type(
                type_annotation,
                &self.type_property_domains,
                &self.type_domains,
                &self.enum_member_domains,
                &self.zod_schema_property_domains,
            ) {
                self.typed_object_property_domains
                    .insert(name.to_string(), properties);
            }
            return;
        }

        let BindingPattern::ObjectPattern(object) = pattern else {
            return;
        };
        let properties = property_domains_from_ts_type(
            type_annotation,
            &self.type_property_domains,
            &self.type_domains,
            &self.enum_member_domains,
            &self.zod_schema_property_domains,
        )
        .unwrap_or_default();
        let type_literal_members = match type_annotation {
            TSType::TSTypeLiteral(literal) => Some(literal.members.as_slice()),
            _ => None,
        };
        for property in &object.properties {
            if property.computed {
                continue;
            }
            let Some(property_name) = property.key.static_name() else {
                continue;
            };
            if let Some(binding_name) = binding_identifier_name(&property.value) {
                if let Some(values) = properties.get(property_name.as_ref()) {
                    self.finite_iterables
                        .insert(binding_name.to_string(), values.clone());
                    self.finite_constants
                        .insert(binding_name.to_string(), values.clone());
                }
                if let Some(property_type) = type_literal_members.and_then(|members| {
                    type_literal_property_annotation(members, property_name.as_ref())
                }) {
                    if let Some(properties) = property_domains_from_ts_type(
                        property_type,
                        &self.type_property_domains,
                        &self.type_domains,
                        &self.enum_member_domains,
                        &self.zod_schema_property_domains,
                    ) {
                        self.typed_object_property_domains
                            .insert(binding_name.to_string(), properties);
                    }
                }
            }
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
            key_start: None,
            key_end: None,
        });
    }

    fn record_dynamic_key_usage(
        &mut self,
        namespace: Option<&str>,
        call_start: u32,
        argument: &Argument<'_>,
    ) {
        let span = argument.span();
        self.scan.dynamic_usages.push(DynamicUsage {
            namespace: namespace.unwrap_or_default().to_string(),
            path: self.path.clone(),
            line: self.line_number(call_start),
            key_start: Some(span.start as usize),
            key_end: Some(span.end as usize),
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
        if let Some(values) = self.finite_strings_from_argument(first) {
            return namespace_arg_from_finite_strings(values);
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
                                if let Some(value) = self.string_from_expression(&property.value) {
                                    return NamespaceArg::Scoped(value);
                                }
                                return self
                                    .finite_strings_from_expression(&property.value)
                                    .map(namespace_arg_from_finite_strings)
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
        match expression.get_inner_expression() {
            Expression::Identifier(identifier) => self
                .finite_iterables
                .get(identifier.name.as_str())
                .cloned()
                .or_else(|| self.finite_iterable_for_imported_identifier(identifier.name.as_str())),
            _ => finite_iterable_from_expression(
                expression,
                &self.finite_constants,
                &self.finite_iterables,
                &self.finite_object_maps,
                &self.enum_member_domains,
            ),
        }
    }

    fn finite_iterable_for_imported_identifier(&self, name: &str) -> Option<FiniteStrings> {
        let target = self.file_index.as_ref()?.imports.get(name)?;
        self.project
            .as_ref()?
            .finite_iterable_for_import(&self.path, target)
    }

    fn finite_strings_for_property_name(&self, property: &str) -> Option<FiniteStrings> {
        let mut values = BTreeSet::new();
        for (name, domain) in &self.finite_iterables {
            if property_names_from_value_constant_name(name).contains(property) {
                values.extend(domain.iter().cloned());
            }
        }
        if let (Some(file_index), Some(project)) = (&self.file_index, &self.project) {
            for (name, target) in &file_index.imports {
                if !property_names_from_value_constant_name(name).contains(property) {
                    continue;
                }
                if let Some(domain) = project.finite_iterable_for_import(&self.path, target) {
                    values.extend(domain);
                }
            }
        }
        if values.is_empty() || values.len() > MAX_FINITE_STRINGS {
            None
        } else {
            Some(values)
        }
    }

    fn finite_record_iterable_from_expression(
        &self,
        expression: &Expression<'_>,
    ) -> Option<FiniteRecords> {
        match expression.get_inner_expression() {
            Expression::Identifier(identifier) => self
                .finite_record_iterables
                .get(identifier.name.as_str())
                .cloned()
                .or_else(|| {
                    let target = self
                        .file_index
                        .as_ref()?
                        .imports
                        .get(identifier.name.as_str())?;
                    self.project
                        .as_ref()?
                        .finite_record_iterable_for_import(&self.path, target)
                }),
            Expression::StaticMemberExpression(member) => {
                let object = member.object.get_identifier_reference()?;
                let records = self.finite_record_constants.get(object.name.as_str())?;
                finite_record_property_iterable(records, member.property.name.as_str())
            }
            Expression::CallExpression(call) => {
                if let Some(member) = call.callee.get_member_expr() {
                    if let Some(method) = member.static_property_name() {
                        let records = self.finite_record_iterable_from_expression(member.object());
                        if let Some(records) = records {
                            return match method.as_ref() {
                                "filter" => Some(records),
                                "slice" => {
                                    let start = call
                                        .arguments
                                        .first()
                                        .and_then(argument_usize)
                                        .unwrap_or(0);
                                    let end = call
                                        .arguments
                                        .get(1)
                                        .and_then(argument_usize)
                                        .unwrap_or(records.len());
                                    Some(
                                        records
                                            .into_iter()
                                            .skip(start)
                                            .take(end.saturating_sub(start))
                                            .collect(),
                                    )
                                }
                                _ => None,
                            };
                        }
                    }
                }
                self.return_record_helper_for_callee(&call.callee)
                    .or_else(|| {
                        finite_record_iterable_from_expression(
                            expression,
                            &self.finite_constants,
                            &self.finite_object_maps,
                            &self.enum_member_domains,
                            &self.finite_record_iterables,
                            &self.finite_record_maps,
                        )
                    })
            }
            _ => finite_record_iterable_from_expression(
                expression,
                &self.finite_constants,
                &self.finite_object_maps,
                &self.enum_member_domains,
                &self.finite_record_iterables,
                &self.finite_record_maps,
            ),
        }
    }

    fn finite_record_from_expression(&self, expression: &Expression<'_>) -> Option<FiniteRecords> {
        self.return_record_helper_for_callee_from_expression(expression)
            .or_else(|| {
                finite_record_from_expression(
                    expression,
                    &self.finite_constants,
                    &self.finite_object_maps,
                    &self.enum_member_domains,
                    &self.finite_record_iterables,
                    &self.finite_record_maps,
                )
            })
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

        Some(translator_binding_from_namespace_arg(
            self.call_namespace(call),
        ))
    }

    fn bind_promise_all_translators(
        &mut self,
        pattern: &BindingPattern<'_>,
        expression: &Expression<'_>,
    ) {
        let BindingPattern::ArrayPattern(pattern) = pattern else {
            return;
        };
        let call = match expression.get_inner_expression() {
            Expression::CallExpression(call) => call,
            Expression::AwaitExpression(await_expression) => {
                match await_expression.argument.get_inner_expression() {
                    Expression::CallExpression(call) => call,
                    _ => return,
                }
            }
            _ => return,
        };
        if !self.is_promise_all_call(call) {
            return;
        }
        let Some(Argument::ArrayExpression(array)) = call.arguments.first() else {
            return;
        };
        for (index, binding_pattern) in pattern.elements.iter().enumerate() {
            let Some(binding_pattern) = binding_pattern else {
                continue;
            };
            let Some(element) = array.elements.get(index) else {
                continue;
            };
            let Some(binding) = self.translator_binding_from_array_element(element) else {
                continue;
            };
            self.bind_translator_pattern(binding_pattern, binding);
        }
    }

    fn is_promise_all_call(&self, call: &CallExpression<'_>) -> bool {
        let Some(member) = call.callee.get_member_expr() else {
            return false;
        };
        member
            .static_property_name()
            .is_some_and(|method| method == "all")
            && is_identifier_expression(member.object(), "Promise")
    }

    fn bind_translator_pattern(
        &mut self,
        pattern: &BindingPattern<'_>,
        binding: TranslatorBinding,
    ) {
        if let Some(name) = binding_identifier_name(pattern) {
            self.translators.insert(name.to_string(), binding);
        }
    }

    fn translator_binding_from_array_element(
        &self,
        element: &ArrayExpressionElement<'_>,
    ) -> Option<TranslatorBinding> {
        match element {
            ArrayExpressionElement::Identifier(identifier) => {
                self.translators.get(identifier.name.as_str()).cloned()
            }
            ArrayExpressionElement::CallExpression(call) => self.translator_binding_from_call(call),
            ArrayExpressionElement::AwaitExpression(await_expression) => {
                self.translator_binding_from_expression(&await_expression.argument)
            }
            ArrayExpressionElement::ParenthesizedExpression(parenthesized) => {
                self.translator_binding_from_expression(&parenthesized.expression)
            }
            ArrayExpressionElement::TSAsExpression(expression) => {
                self.translator_binding_from_expression(&expression.expression)
            }
            ArrayExpressionElement::TSSatisfiesExpression(expression) => {
                self.translator_binding_from_expression(&expression.expression)
            }
            ArrayExpressionElement::TSNonNullExpression(expression) => {
                self.translator_binding_from_expression(&expression.expression)
            }
            ArrayExpressionElement::TSInstantiationExpression(expression) => {
                self.translator_binding_from_expression(&expression.expression)
            }
            ArrayExpressionElement::TSTypeAssertion(expression) => {
                self.translator_binding_from_expression(&expression.expression)
            }
            _ => None,
        }
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

    fn return_record_helper_for_callee(
        &self,
        expression: &Expression<'_>,
    ) -> Option<FiniteRecords> {
        let callee = self.callee_identifier(expression)?;
        if let Some(file_index) = &self.file_index {
            if let Some(summary) = file_index.return_record_helper_for_local(callee) {
                return Some(summary);
            }
            if let Some(target) = file_index.imports.get(callee) {
                if let Some(project) = &self.project {
                    return project.return_record_helper_for_import(&self.path, target);
                }
            }
        }
        None
    }

    fn return_record_helper_for_callee_from_expression(
        &self,
        expression: &Expression<'_>,
    ) -> Option<FiniteRecords> {
        let Expression::CallExpression(call) = expression.get_inner_expression() else {
            return None;
        };
        self.return_record_helper_for_callee(&call.callee)
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

        Some(translator_binding_from_namespace_arg(
            self.call_namespace(call),
        ))
    }

    fn record_used_key_for_binding(&mut self, binding: &TranslatorBinding, key: &str) {
        if let Some(namespaces) = &binding.namespaces {
            for namespace in namespaces {
                let id = if key.is_empty() {
                    namespace.clone()
                } else {
                    format!("{namespace}.{key}")
                };
                self.scan.used_ids.insert(id);
            }
            return;
        }

        let id = match &binding.namespace {
            Some(namespace) if key.is_empty() => namespace.clone(),
            Some(namespace) => format!("{namespace}.{key}"),
            None => key.to_string(),
        };
        self.scan.used_ids.insert(id);
    }

    fn record_message_key_helper_call(&mut self, call: &CallExpression<'_>) -> bool {
        let Some(callee) = self.callee_identifier(&call.callee) else {
            return false;
        };
        let Some(param_index) = self.message_key_helpers.get(callee).copied() else {
            return false;
        };
        let Some(argument) = call.arguments.get(param_index) else {
            return true;
        };
        if let Some(keys) = self.finite_strings_from_argument(argument) {
            self.scan.used_ids.extend(keys);
        } else {
            self.record_dynamic_key_usage(None, call.span.start, argument);
        }
        true
    }

    fn record_dynamic_key_for_binding(
        &mut self,
        binding: &TranslatorBinding,
        start: u32,
        argument: Option<&Argument<'_>>,
    ) {
        if let Some(namespaces) = &binding.namespaces {
            let namespaces: Vec<_> = namespaces.iter().cloned().collect();
            for namespace in namespaces {
                if let Some(argument) = argument {
                    self.record_dynamic_key_usage(Some(&namespace), start, argument);
                } else {
                    self.record_dynamic_usage(Some(&namespace), start);
                }
            }
            return;
        }

        if let Some(argument) = argument {
            self.record_dynamic_key_usage(binding.namespace.as_deref(), start, argument);
        } else {
            self.record_dynamic_usage(binding.namespace.as_deref(), start);
        }
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
                        self.record_used_key_for_binding(&binding, key);
                    }
                }
                _ => self.record_dynamic_key_for_binding(&binding, call.span.start, None),
            }
        }
    }

    fn protect_translator_arguments(&mut self, call: &CallExpression<'_>) {
        for argument in &call.arguments {
            if let Some(binding) = self.translator_binding_from_argument(argument) {
                self.record_dynamic_key_for_binding(&binding, call.span.start, None);
            }
        }
    }

    fn finite_strings_from_argument(&self, argument: &Argument<'_>) -> Option<FiniteStrings> {
        match argument {
            Argument::CallExpression(call) => self.finite_strings_from_call(call),
            Argument::TemplateLiteral(literal) => self.finite_strings_from_template(literal),
            Argument::StaticMemberExpression(member) => self
                .finite_strings_from_record_member(&member.object, member.property.name.as_str())
                .or_else(|| self.finite_strings_for_property_name(member.property.name.as_str()))
                .or_else(|| {
                    finite_strings_from_argument(
                        argument,
                        &self.finite_constants,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                    )
                }),
            _ => finite_strings_from_argument(
                argument,
                &self.finite_constants,
                &self.finite_object_maps,
                &self.enum_member_domains,
            ),
        }
    }

    fn finite_strings_from_expression(&self, expression: &Expression<'_>) -> Option<FiniteStrings> {
        match expression.get_inner_expression() {
            Expression::CallExpression(call) => self.finite_strings_from_call(call),
            Expression::TemplateLiteral(literal) => self.finite_strings_from_template(literal),
            Expression::StaticMemberExpression(member) => self
                .finite_strings_from_record_member(&member.object, member.property.name.as_str())
                .or_else(|| self.finite_strings_for_property_name(member.property.name.as_str()))
                .or_else(|| {
                    finite_strings_from_expression(
                        expression,
                        &self.finite_constants,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                    )
                }),
            _ => finite_strings_from_expression(
                expression,
                &self.finite_constants,
                &self.finite_object_maps,
                &self.enum_member_domains,
            ),
        }
    }

    fn finite_strings_from_template(&self, literal: &TemplateLiteral<'_>) -> Option<FiniteStrings> {
        let mut values = finite_string("");
        for (index, quasi) in literal.quasis.iter().enumerate() {
            let cooked = quasi.value.cooked.as_ref()?;
            values = append_finite_strings(values, &finite_string(cooked.to_string()))?;

            if let Some(expression) = literal.expressions.get(index) {
                let expression_values = self.finite_strings_from_expression(expression)?;
                values = append_finite_strings(values, &expression_values)?;
            }
        }
        Some(values)
    }

    fn finite_strings_from_record_member(
        &self,
        object: &Expression<'_>,
        property: &str,
    ) -> Option<FiniteStrings> {
        let object = object.get_identifier_reference()?;
        if let Some(records) = self.finite_record_constants.get(object.name.as_str()) {
            if let Some(values) = finite_record_property_strings(records, property) {
                return Some(values);
            }
        }
        self.typed_object_property_domains
            .get(object.name.as_str())?
            .get(property)
            .cloned()
    }

    fn finite_strings_from_call(&self, call: &CallExpression<'_>) -> Option<FiniteStrings> {
        if let Some(values) = self.finite_strings_from_typed_property_call(call) {
            return Some(values);
        }
        if call.arguments.is_empty() {
            if let Some(member) = call.callee.get_member_expr() {
                if let Some(method) = member.static_property_name() {
                    if let Some(values) = self.finite_strings_from_expression(member.object()) {
                        if let Some(values) = transform_finite_strings(values, &method) {
                            return Some(values);
                        }
                    }
                }
            }
        }
        self.return_helper_for_callee(&call.callee).or_else(|| {
            finite_strings_from_call(
                call,
                &self.finite_constants,
                &self.finite_object_maps,
                &self.enum_member_domains,
            )
        })
    }

    fn finite_strings_from_typed_property_call(
        &self,
        call: &CallExpression<'_>,
    ) -> Option<FiniteStrings> {
        let member = call.callee.get_member_expr()?;
        if member.static_property_name()? != "watch" {
            return None;
        }
        let object = member.object().get_identifier_reference()?;
        let properties = self
            .typed_object_property_domains
            .get(object.name.as_str())?;
        let property = call.arguments.first().and_then(|argument| {
            finite_strings_from_argument(
                argument,
                &self.finite_constants,
                &self.finite_object_maps,
                &self.enum_member_domains,
            )
            .and_then(|values| single_finite_string(&values))
        })?;
        properties.get(&property).cloned()
    }

    fn property_domains_from_expression(
        &self,
        expression: &Expression<'_>,
    ) -> Option<PropertyDomains> {
        match expression.get_inner_expression() {
            Expression::Identifier(identifier) => self
                .typed_object_property_domains
                .get(identifier.name.as_str())
                .cloned(),
            Expression::CallExpression(call) => self.property_domains_from_call(call),
            Expression::LogicalExpression(logical)
                if logical.operator == LogicalOperator::Coalesce =>
            {
                let left = self.property_domains_from_expression(&logical.left);
                let right = self.property_domains_from_expression(&logical.right);
                match (left, right) {
                    (Some(left), Some(right)) => merge_property_domains(left, right),
                    (Some(values), None) | (None, Some(values)) => Some(values),
                    (None, None) => None,
                }
            }
            Expression::TSAsExpression(expression) => property_domains_from_ts_type(
                &expression.type_annotation,
                &self.type_property_domains,
                &self.type_domains,
                &self.enum_member_domains,
                &self.zod_schema_property_domains,
            )
            .or_else(|| self.property_domains_from_expression(&expression.expression)),
            Expression::TSSatisfiesExpression(expression) => property_domains_from_ts_type(
                &expression.type_annotation,
                &self.type_property_domains,
                &self.type_domains,
                &self.enum_member_domains,
                &self.zod_schema_property_domains,
            )
            .or_else(|| self.property_domains_from_expression(&expression.expression)),
            Expression::TSNonNullExpression(expression) => {
                self.property_domains_from_expression(&expression.expression)
            }
            Expression::TSTypeAssertion(expression) => property_domains_from_ts_type(
                &expression.type_annotation,
                &self.type_property_domains,
                &self.type_domains,
                &self.enum_member_domains,
                &self.zod_schema_property_domains,
            )
            .or_else(|| self.property_domains_from_expression(&expression.expression)),
            _ => None,
        }
    }

    fn property_domains_from_call(&self, call: &CallExpression<'_>) -> Option<PropertyDomains> {
        let member = call.callee.get_member_expr()?;
        if !call.arguments.is_empty() || member.static_property_name()? != "getValues" {
            return None;
        }
        let object = member.object().get_identifier_reference()?;
        self.typed_object_property_domains
            .get(object.name.as_str())
            .cloned()
    }

    fn property_domains_from_call_type_arguments(
        &self,
        expression: &Expression<'_>,
    ) -> Option<BTreeMap<String, FiniteStrings>> {
        let Expression::CallExpression(call) = expression.get_inner_expression() else {
            return None;
        };
        let first_type_argument = call.type_arguments.as_ref()?.params.first()?;
        property_domains_from_ts_type(
            first_type_argument,
            &self.type_property_domains,
            &self.type_domains,
            &self.enum_member_domains,
            &self.zod_schema_property_domains,
        )
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

    fn visit_finite_record_iteration_callback(
        &mut self,
        callback: &Argument<'_>,
        values: FiniteRecords,
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
                self.with_finite_record_constant(param, values, |collector| {
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
                self.with_finite_record_constant(param, values, |collector| {
                    walk::walk_function(collector, function, ScopeFlags::Function);
                });
                true
            }
            _ => false,
        }
    }

    fn visit_finite_record_iteration_call(&mut self, call: &CallExpression<'_>) -> bool {
        let Some(member) = call.callee.get_member_expr() else {
            return false;
        };
        let Some(method) = member.static_property_name() else {
            return false;
        };
        if !matches!(method.as_ref(), "map" | "forEach") {
            return false;
        }
        let Some(values) = self.finite_record_iterable_from_expression(member.object()) else {
            return false;
        };
        let Some(callback) = call.arguments.first() else {
            return false;
        };
        self.visit_finite_record_iteration_callback(callback, values)
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

    fn with_finite_record_constant(
        &mut self,
        name: &str,
        values: FiniteRecords,
        visit: impl FnOnce(&mut Self),
    ) {
        let previous = self
            .finite_record_constants
            .insert(name.to_string(), values);
        visit(self);
        match previous {
            Some(values) => {
                self.finite_record_constants
                    .insert(name.to_string(), values);
            }
            None => {
                self.finite_record_constants.remove(name);
            }
        }
    }

    fn bind_pattern_finite_record_properties(
        &mut self,
        pattern: &BindingPattern<'_>,
        records: &FiniteRecords,
    ) {
        let BindingPattern::ObjectPattern(object) = pattern else {
            return;
        };
        for property in &object.properties {
            if property.computed {
                continue;
            }
            let Some(property_name) = property.key.static_name() else {
                continue;
            };
            let Some(binding_name) = binding_identifier_name(&property.value) else {
                continue;
            };
            if let Some(values) = finite_record_property_strings(records, property_name.as_ref()) {
                self.finite_constants
                    .insert(binding_name.to_string(), values.clone());
                self.finite_iterables
                    .insert(binding_name.to_string(), values);
            }
            if let Some(values) = finite_record_property_iterable(records, property_name.as_ref()) {
                self.finite_record_iterables
                    .insert(binding_name.to_string(), values);
            }
        }
    }
}

impl<'a> Visit<'a> for SourceUsageCollector {
    fn visit_ts_type_alias_declaration(&mut self, declaration: &TSTypeAliasDeclaration<'a>) {
        self.record_ts_type_alias(declaration);
        walk::walk_ts_type_alias_declaration(self, declaration);
    }

    fn visit_ts_interface_declaration(&mut self, declaration: &TSInterfaceDeclaration<'a>) {
        self.record_ts_interface(declaration);
        walk::walk_ts_interface_declaration(self, declaration);
    }

    fn visit_formal_parameter(&mut self, parameter: &oxc_ast::ast::FormalParameter<'a>) {
        if let Some(annotation) = &parameter.type_annotation {
            self.bind_pattern_type_domains(&parameter.pattern, &annotation.type_annotation);
        }
        walk::walk_formal_parameter(self, parameter);
    }

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

    fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
        if let Some(id) = &function.id {
            if let Some(param_index) = message_key_helper_from_function(function) {
                self.message_key_helpers
                    .insert(id.name.to_string(), param_index);
            }
        }
        walk::walk_function(self, function, flags);
    }

    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        if declarator.kind == VariableDeclarationKind::Const {
            if let Some(init) = &declarator.init {
                if let Some(records) = self.finite_record_from_expression(init) {
                    self.bind_pattern_finite_record_properties(&declarator.id, &records);
                }
            }
        }

        if let Some(init) = &declarator.init {
            self.bind_promise_all_translators(&declarator.id, init);
        }

        if let BindingPattern::BindingIdentifier(identifier) = &declarator.id {
            let name = identifier.name.as_str();
            if declarator.kind == VariableDeclarationKind::Const {
                if let Some(init) = &declarator.init {
                    record_enum_member_domains_from_expression(init, &mut self.enum_member_domains);
                    if let Some(annotation) = &declarator.type_annotation {
                        self.bind_pattern_type_domains(&declarator.id, &annotation.type_annotation);
                    }
                    if let Some(properties) = self.zod_schema_property_domains_from_expression(init)
                    {
                        self.zod_schema_property_domains
                            .insert(name.to_string(), properties);
                    }
                    if let Some(values) = self.finite_strings_from_expression(init) {
                        self.finite_constants.insert(name.to_string(), values);
                    }
                    if let Some(values) = self.finite_iterable_from_expression(init) {
                        self.finite_iterables.insert(name.to_string(), values);
                    }
                    if let Some(values) = finite_object_map_from_expression(
                        init,
                        &self.finite_constants,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                    ) {
                        self.finite_object_maps.insert(name.to_string(), values);
                    }
                    if let Some(values) = self.finite_record_iterable_from_expression(init) {
                        self.finite_record_iterables
                            .insert(name.to_string(), values);
                    }
                    if let Some(values) = self.return_record_helper_for_callee_from_expression(init)
                    {
                        self.finite_record_constants
                            .insert(name.to_string(), values);
                    } else if let Some(values) = finite_record_from_expression(
                        init,
                        &self.finite_constants,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                        &self.finite_record_iterables,
                        &self.finite_record_maps,
                    ) {
                        self.finite_record_constants
                            .insert(name.to_string(), values);
                    }
                    if let Some(values) = finite_record_map_from_expression(
                        init,
                        &self.finite_constants,
                        &self.finite_object_maps,
                        &self.enum_member_domains,
                        &self.finite_record_iterables,
                        &self.finite_record_maps,
                    ) {
                        self.finite_record_maps.insert(name.to_string(), values);
                    }
                    if let Some(properties) = self.property_domains_from_call_type_arguments(init) {
                        self.typed_object_property_domains
                            .insert(name.to_string(), properties);
                    }
                    if let Some(properties) = self.property_domains_from_expression(init) {
                        self.typed_object_property_domains
                            .insert(name.to_string(), properties);
                    }
                }
            } else if let Some(annotation) = &declarator.type_annotation {
                self.bind_pattern_type_domains(&declarator.id, &annotation.type_annotation);
            }

            if let Some(init) = &declarator.init {
                if let Some(binding) = self.translator_binding_from_expression(init) {
                    self.translators.insert(name.to_string(), binding);
                } else if self.maybe_translation_factory(init).unwrap_or(false) {
                    self.record_dynamic_usage(None, init.span().start);
                }
                if let Some(param_index) = message_key_helper_from_expression(init) {
                    self.message_key_helpers
                        .insert(name.to_string(), param_index);
                }
            }
        }

        walk::walk_variable_declarator(self, declarator);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if self.maybe_extraction_call(call) {
            self.record_extraction_usage();
        }

        self.record_message_key_helper_call(call);

        if self.visit_finite_record_iteration_call(call) {
            if let Some(member) = call.callee.get_member_expr() {
                self.visit_expression(member.object());
            }
            return;
        }

        if self.visit_finite_iteration_call(call) {
            if let Some(member) = call.callee.get_member_expr() {
                self.visit_expression(member.object());
            }
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
                        self.record_used_key_for_binding(&binding, &key);
                    }
                }
                _ => {
                    if !binding.dynamic_namespace {
                        if let Some(argument) = call.arguments.first() {
                            self.record_dynamic_key_for_binding(
                                &binding,
                                call.span.start,
                                Some(argument),
                            );
                        } else {
                            self.record_dynamic_key_for_binding(&binding, call.span.start, None);
                        }
                    } else {
                        self.record_dynamic_key_for_binding(&binding, call.span.start, None);
                    }
                }
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
        if let Some(values) = self.finite_record_iterable_from_expression(&statement.right) {
            if let Some(binding) = self.for_of_binding_name(statement).map(str::to_string) {
                self.with_finite_record_constant(&binding, values, |collector| {
                    collector.visit_statement(&statement.body);
                });
                return;
            }
        }

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
    fn ts_checker_queries_include_unscoped_dynamic_key_usage() {
        let queries = ts_checker_queries(&[DynamicUsage {
            namespace: String::new(),
            path: PathBuf::from("/tmp/sample.tsx"),
            line: 1,
            key_start: Some(10),
            key_end: Some(20),
        }]);

        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].namespace, "");
        assert_eq!(queries[0].start, 10);
        assert_eq!(queries[0].end, 20);
    }

    #[test]
    fn resolved_translation_id_handles_scoped_and_unscoped_keys() {
        assert_eq!(
            resolved_translation_id("", "common.save"),
            Some("common.save".to_string())
        );
        assert_eq!(
            resolved_translation_id("common", "save"),
            Some("common.save".to_string())
        );
        assert_eq!(
            resolved_translation_id("common", ""),
            Some("common".to_string())
        );
        assert_eq!(resolved_translation_id("", ""), None);
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
    fn collects_promise_all_destructured_get_translations() {
        let scan = scan(
            r#"
            import {getTranslations} from 'next-intl/server';
            const [{tenant}, tDashboard, , tProjects, tEmptyPages] = await Promise.all([
              params,
              getTranslations('dashboard'),
              getLocale(),
              getTranslations({locale, namespace: 'projects.checklists'}),
              getTranslations(
                activeTab === 'templates'
                  ? 'empty-pages.checklistTemplates'
                  : 'empty-pages.checklists',
              ),
            ]);
            tDashboard('labels.offer');
            tProjects('ongoing');
            tEmptyPages('header');
            "#,
        );
        assert!(scan.used_ids.contains("dashboard.labels.offer"));
        assert!(scan.used_ids.contains("projects.checklists.ongoing"));
        assert!(
            scan.used_ids
                .contains("empty-pages.checklistTemplates.header")
        );
        assert!(scan.used_ids.contains("empty-pages.checklists.header"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn traces_helper_from_promise_all_destructured_get_translations() {
        let scan = scan(
            r#"
            import {getTranslations} from 'next-intl/server';
            function buildCopy(translate) {
              translate('sections.overview');
            }
            const [tDashboard] = await Promise.all([
              getTranslations('dashboard'),
            ]);
            buildCopy(tDashboard);
            "#,
        );
        assert!(scan.used_ids.contains("dashboard.sections.overview"));
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
    fn resolves_typed_finite_union_keys() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            type Category = 'REGULAR' | 'ABSENCE';
            const t = useTranslations('settings');
            const category: Category = getCategory();
            t(`timeTypes.categories.${category}`);
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
    fn resolves_enum_like_member_iterable_keys() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const TenantUserType = {
              ADMIN: 'ADMIN',
              USER: 'USER',
            };
            const USER_TYPES = [TenantUserType.ADMIN, TenantUserType.USER] as const;
            const t = useTranslations('users');
            USER_TYPES.map(userType => t(`user-types.${userType}`));
            "#,
        );
        assert!(scan.used_ids.contains("users.user-types.ADMIN"));
        assert!(scan.used_ids.contains("users.user-types.USER"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_property_values_from_matching_finite_value_constants() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const VARIABLE_TYPES = ['NUMBER', 'TEXT'] as const;
            const t = useTranslations('template');
            t(`variables.types.${variable.type.toLowerCase()}`);
            "#,
        );
        assert!(scan.used_ids.contains("template.variables.types.number"));
        assert!(scan.used_ids.contains("template.variables.types.text"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_typed_use_form_watch_keys() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            import {useForm} from 'react-hook-form';
            type Category = 'REGULAR' | 'ABSENCE';
            type TimeTypeFormValues = {
              category: Category;
            };
            const t = useTranslations('settings');
            const form = useForm<TimeTypeFormValues>();
            const category = form.watch('category');
            t(`timeTypes.categories.${category}`);
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
    fn resolves_destructured_typed_iterable_props() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const Schema = z.enum([TenantUserType.ADMIN, TenantUserType.USER]);
            interface Props {
              allowedUserTypes: readonly TenantUserType[];
            }
            function Component({allowedUserTypes}: Props) {
              const t = useTranslations('users');
              return allowedUserTypes.map(userType => t(`user-types.${userType}`));
            }
            "#,
        );
        assert!(scan.used_ids.contains("users.user-types.ADMIN"));
        assert!(scan.used_ids.contains("users.user-types.USER"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_zod_inferred_use_form_watch_keys() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            import {useForm} from 'react-hook-form';
            import {z} from 'zod';
            const RELATION_VALUES = ['parent', 'partner'] as const;
            const EmergencyContactFormSchema = z
              .object({
                relation: z.enum(RELATION_VALUES),
              })
              .superRefine(() => {});
            type EmergencyContactFormValues = z.infer<typeof EmergencyContactFormSchema>;
            const form = useForm<EmergencyContactFormValues>();
            const relation = form.watch('relation');
            const tRelation = useTranslations('users.relations');
            tRelation(relation);
            "#,
        );
        assert!(scan.used_ids.contains("users.relations.parent"));
        assert!(scan.used_ids.contains("users.relations.partner"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_zod_inferred_use_watch_value_properties() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            import {UseFormReturn, useWatch} from 'react-hook-form';
            import {z} from 'zod';
            const RELATION_VALUES = ['parent', 'partner'] as const;
            const EmergencyContactFormSchema = z.object({
              relation: z.enum(RELATION_VALUES),
            });
            type EmergencyContactFormValues = z.infer<typeof EmergencyContactFormSchema>;
            function Fields({form}: {form: UseFormReturn<EmergencyContactFormValues>}) {
              const watchedValues = useWatch({
                control: form.control,
              }) as EmergencyContactFormValues | undefined;
              const values = watchedValues ?? form.getValues();
              const tRelation = useTranslations('users.relations');
              tRelation(values.relation);
            }
            "#,
        );
        assert!(scan.used_ids.contains("users.relations.parent"));
        assert!(scan.used_ids.contains("users.relations.partner"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_finite_object_map_unknown_lookup_values() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations();
            const messageKeyByReason = {
              blocked: 'notifications.errors.blocked',
              denied: 'notifications.errors.denied',
              registrationFailed: 'notifications.errors.registrationFailed',
              unsupported: 'notifications.errors.unsupported',
            } as const;
            t(messageKeyByReason[error.reason]);
            "#,
        );
        assert!(scan.used_ids.contains("notifications.errors.blocked"));
        assert!(scan.used_ids.contains("notifications.errors.denied"));
        assert!(
            scan.used_ids
                .contains("notifications.errors.registrationFailed")
        );
        assert!(scan.used_ids.contains("notifications.errors.unsupported"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_finite_object_map_specific_lookup_values() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations('notifications');
            const reason = failed ? 'registrationFailed' : 'unsupported';
            const messageKeyByReason = {
              blocked: 'errors.blocked',
              denied: 'errors.denied',
              registrationFailed: 'errors.registrationFailed',
              unsupported: 'errors.unsupported',
            };
            t(messageKeyByReason[reason]);
            t(messageKeyByReason.blocked);
            "#,
        );
        assert!(scan.used_ids.contains("notifications.errors.blocked"));
        assert!(!scan.used_ids.contains("notifications.errors.denied"));
        assert!(
            scan.used_ids
                .contains("notifications.errors.registrationFailed")
        );
        assert!(scan.used_ids.contains("notifications.errors.unsupported"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn unresolved_object_map_stays_dynamic() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const t = useTranslations('notifications');
            const messageKeyByReason = {
              blocked: 'errors.blocked',
              ...extraReasons,
            };
            t(messageKeyByReason[error.reason]);
            "#,
        );
        assert!(
            scan.dynamic_usages
                .iter()
                .any(|usage| usage.namespace == "notifications")
        );
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
    fn traces_helper_finite_object_map_keys() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            function helper(tx, reason) {
              const messageKeyByReason = {
                blocked: 'notifications.errors.blocked',
                denied: 'notifications.errors.denied',
              };
              tx(messageKeyByReason[reason]);
            }
            const t = useTranslations();
            helper(t, reason);
            "#,
        );
        assert!(scan.used_ids.contains("notifications.errors.blocked"));
        assert!(scan.used_ids.contains("notifications.errors.denied"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn traces_helper_keys_from_module_scope_object_map() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const labelKeyByStatus = {
              CREATED: 'status-created',
              SENT: 'status-sent',
            } as const;
            function getStatusLabel(status, translate) {
              const labelKey = status === 'SENT' ? 'status-opened' : labelKeyByStatus[status];
              return translate(labelKey);
            }
            const t = useTranslations('offers');
            getStatusLabel(status, t);
            "#,
        );
        assert!(scan.used_ids.contains("offers.status-created"));
        assert!(scan.used_ids.contains("offers.status-sent"));
        assert!(scan.used_ids.contains("offers.status-opened"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn traces_helper_keys_from_module_scope_record_lookup() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const statusViewByStatus = {
              SENT: {labelKey: 'status.sent', className: 'sent'},
              DECLINED: {labelKey: 'status.declined', className: 'declined'},
            } as const;
            const openedStatusView = {labelKey: 'opened', className: 'opened'} as const;
            const draftStatusView = {labelKey: 'status.draft', className: 'draft'} as const;
            function getStatusView(status, translate, firstOpenedAt) {
              const view =
                status === 'SENT' && firstOpenedAt
                  ? openedStatusView
                  : (statusViewByStatus[status] ?? draftStatusView);
              return translate(view.labelKey);
            }
            const t = useTranslations('projects.offers');
            getStatusView(status, t, firstOpenedAt);
            "#,
        );
        assert!(scan.used_ids.contains("projects.offers.status.sent"));
        assert!(scan.used_ids.contains("projects.offers.status.declined"));
        assert!(scan.used_ids.contains("projects.offers.opened"));
        assert!(scan.used_ids.contains("projects.offers.status.draft"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn traces_nested_translator_helpers() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            function getTypeLabel(item, translate) {
              return item.type === 'STOPWATCH'
                ? translate('timesheets.stopwatch')
                : translate('timesheets.hours-ordinary');
            }
            function getContextLabel(item, translate) {
              return getTypeLabel(item, translate);
            }
            const t = useTranslations();
            getContextLabel(item, t);
            "#,
        );
        assert!(scan.used_ids.contains("timesheets.stopwatch"));
        assert!(scan.used_ids.contains("timesheets.hours-ordinary"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn traces_helper_calls_used_as_chained_receiver() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const getOptions = translate => [
              {label: translate('common.time-entry')},
              {label: translate('checklists.create-menu-label')},
            ];
            const t = useTranslations();
            getOptions(t).map(option => option.label);
            "#,
        );
        assert!(scan.used_ids.contains("common.time-entry"));
        assert!(scan.used_ids.contains("checklists.create-menu-label"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn traces_local_get_message_label_helpers() {
        let scan = scan(
            r#"
            function getMessage(messages, key) {
              return messages[key];
            }
            const label = (key) => {
              const value = getMessage(messages, key);
              return typeof value === 'string' ? value : key;
            };
            const labels = {
              dateCreated: label('checklists.preview.date-created'),
              item: label('common.item'),
            };
            "#,
        );
        assert!(scan.used_ids.contains("checklists.preview.date-created"));
        assert!(scan.used_ids.contains("common.item"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_finite_record_map_callback_properties() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const ITEMS = [
              {labelKey: 'navigation.profile'},
              {labelKey: 'navigation.members'},
            ] as const;
            const t = useTranslations('settings');
            ITEMS.map(item => t(item.labelKey));
            "#,
        );
        assert!(scan.used_ids.contains("settings.navigation.profile"));
        assert!(scan.used_ids.contains("settings.navigation.members"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_finite_record_return_helper_nested_properties() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const ITEMS = [
              {labelKey: 'navigation.profile'},
              {labelKey: 'navigation.members'},
              {labelKey: 'navigation.auditLog'},
            ] as const;
            function getSections(isDeveloper) {
              const sections = [
                {
                  labelKey: 'navigation.sections.account',
                  items: ITEMS.slice(0, 2),
                },
              ];
              if (isDeveloper) {
                sections.push({
                  labelKey: 'navigation.sections.system',
                  items: ITEMS.slice(2),
                });
              }
              return sections;
            }
            const t = useTranslations('settings');
            const sections = getSections(isDeveloper);
            sections.map(section => {
              t(section.labelKey);
              section.items.forEach(item => t(item.labelKey));
            });
            "#,
        );
        assert!(
            scan.used_ids
                .contains("settings.navigation.sections.account")
        );
        assert!(
            scan.used_ids
                .contains("settings.navigation.sections.system")
        );
        assert!(scan.used_ids.contains("settings.navigation.profile"));
        assert!(scan.used_ids.contains("settings.navigation.members"));
        assert!(scan.used_ids.contains("settings.navigation.auditLog"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_filtered_finite_record_iterable_properties() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            type Option = {value: string; labelKey: string};
            const OPTIONS: Option[] = [
              {value: 'all', labelKey: 'filters.all'},
              {value: 'mine', labelKey: 'filters.mine'},
            ];
            const t = useTranslations();
            OPTIONS.filter(option => option.value !== 'all').map(option => t(option.labelKey));
            "#,
        );
        assert!(scan.used_ids.contains("filters.all"));
        assert!(scan.used_ids.contains("filters.mine"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_record_indexed_return_helper_properties() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const EVENT_TYPE_CONFIG = {
              CREATED: {titleKey: 'offers.history.event-created'},
              SENT: {titleKey: 'offers.history.event-sent'},
            };
            function getEventConfig(eventType) {
              return EVENT_TYPE_CONFIG[eventType];
            }
            const t = useTranslations();
            const config = getEventConfig(eventType);
            t(config.titleKey);
            "#,
        );
        assert!(scan.used_ids.contains("offers.history.event-created"));
        assert!(scan.used_ids.contains("offers.history.event-sent"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_destructured_finite_record_iterable_from_hook_return() {
        let scan = scan(
            r#"
            import {useMemo} from 'react';
            import {useTranslations} from 'next-intl';
            const ITEMS = [
              {id: 'home', labelKey: 'common.home'},
              {id: 'offers', labelKey: 'common.offers'},
            ];
            function useFooterNavigationPreferences(itemIds) {
              const visibleItems = useMemo(
                () => ITEMS.filter(item => itemIds.includes(item.id)),
                [itemIds],
              );
              return {visibleItems};
            }
            const t = useTranslations();
            const {visibleItems} = useFooterNavigationPreferences(itemIds);
            visibleItems.map(item => t(item.labelKey));
            "#,
        );
        assert!(scan.used_ids.contains("common.home"));
        assert!(scan.used_ids.contains("common.offers"));
        assert!(scan.dynamic_usages.is_empty());
    }

    #[test]
    fn resolves_finite_record_map_get_properties() {
        let scan = scan(
            r#"
            import {useTranslations} from 'next-intl';
            const ITEMS = [
              {id: 'profile', labelKey: 'navigation.profile'},
              {id: 'members', labelKey: 'navigation.members'},
            ] as const;
            const ITEM_BY_ID = new Map(ITEMS.map(item => [item.id, item]));
            function getItemById(id) {
              return ITEM_BY_ID.get(id) ?? null;
            }
            const t = useTranslations('settings');
            const item = getItemById(id);
            if (item) {
              t(item.labelKey);
            }
            "#,
        );
        assert!(scan.used_ids.contains("settings.navigation.profile"));
        assert!(scan.used_ids.contains("settings.navigation.members"));
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
