"""Prepare bounded-workload API requests from an explicitly selected RDF directory.

This module discovers files and preserves their bytes; OntoKiln remains the
RDF, rule and SPARQL parser. Discovery never supplies business parameters or
interprets application configuration. Inventory contains metadata, not queries.
"""
import fnmatch
import hashlib
import json
import os
from pathlib import Path
import re
import stat


CONTRACT = "ontokiln-api/v1"
SCHEMA = "ontokiln-business-repository/v1"
RDF_FORMATS = {".ttl": "turtle", ".nt": "ntriples", ".trig": "trig", ".nq": "nquads"}
TECHNICAL_DIRECTORIES = {".git", ".hg", ".svn", ".venv", "venv", "node_modules", "__pycache__", "target"}
UNSUPPORTED_RDF_SUFFIXES = {".rdf", ".owl", ".xml", ".jsonld", ".json-ld", ".n3", ".trix", ".rdfs"}
PROFILES = {"no-rules-v1", "datalog-v1", "rdfs-supported-v1", "rdfs-property-chain-v1"}
CONSTRUCT_MODE = "INDEPENDENT_REQUEST_LOCAL_CONSTRUCT_PROBE"
PARAM_NAMESPACE = "urn:ontokiln:param:"
SUPPORTED_QUERY_FORMS = {"SELECT", "ASK", "CONSTRUCT"}
UNSUPPORTED_QUERY_FORMS = {"DESCRIBE", "INSERT", "DELETE", "WITH", "LOAD", "CLEAR", "CREATE", "DROP", "COPY", "MOVE", "ADD"}
IRI_TOKEN = re.compile(r'<(?:[^<>\x00-\x20{}"|^`\\]|\\u[0-9a-fA-F]{4}|\\U[0-9a-fA-F]{8})*>')
WORD_TOKEN = re.compile(r"(?:[^\W\d]|_)[\w:.-]*|:[\w:.-]*", re.UNICODE)
VARIABLE_TOKEN = re.compile(r"[?$](?:[^\W\d]|_)[\w]*", re.UNICODE)
TEMPLATE_SLOT = re.compile(r"\$\{[^{}\r\n]+\}|\{\{\s*[A-Za-z_][\w.:-]*(?:\s*\|[^{}\r\n]+)?\s*\}\}")


def _within(path, parent):
    try:
        path.relative_to(parent)
        return True
    except ValueError:
        return False


def _matches(relative, patterns, directory):
    for pattern in patterns:
        while True:
            if fnmatch.fnmatchcase(relative, pattern) or (directory and fnmatch.fnmatchcase(relative + "/", pattern)):
                return True
            if not pattern.startswith("**/"):
                break
            pattern = pattern[3:]
    return False


def _scan(root, work_dir, generated, excludes, inventory, scope):
    files = []

    def excluded(path, directory):
        relative = path.relative_to(root).as_posix()
        if path.is_symlink():
            reason = "SYMLINK_NOT_FOLLOWED"
        elif path.name in TECHNICAL_DIRECTORIES:
            reason = "TECHNICAL_DIRECTORY"
        elif _within(path, work_dir):
            reason = "WORK_DIRECTORY"
        elif _within(path, generated):
            reason = "GENERATED_DIRECTORY"
        elif _matches(relative, excludes, directory):
            reason = "EXCLUDE_GLOB"
        elif not directory and not stat.S_ISREG(path.lstat().st_mode):
            reason = "NON_REGULAR_FILE"
        else:
            return False
        inventory["excluded_paths"].append({"path": str(path), "relative_path": relative,
                                            "scope": scope, "reason_code": reason})
        return True

    if excluded(root, True):
        return files

    def walk_error(error):
        raise error

    for current, directories, names in os.walk(root, topdown=True, followlinks=False, onerror=walk_error):
        current = Path(current)
        directories[:] = [name for name in sorted(directories) if not excluded(current / name, True)]
        for name in sorted(names):
            path = current / name
            if not excluded(path, False):
                files.append(path)
    return sorted(files, key=lambda path: path.relative_to(root).as_posix())


def _tokens(text):
    """Recognize a query prologue and readiness markers, not SPARQL grammar."""
    result, visible, index = [], list(text), 0
    while index < len(text):
        char = text[index]
        if char.isspace() or char == "\ufeff":
            index += 1
            continue
        if char == "#":
            end = text.find("\n", index)
            end = len(text) if end < 0 else end
            visible[index:end] = " " * (end - index)
            index = end
            continue
        if char in "\"'":
            delimiter = char * 3 if text.startswith(char * 3, index) else char
            end = index + len(delimiter)
            while end < len(text):
                if text[end] == "\\":
                    end += 2
                elif text.startswith(delimiter, end):
                    end += len(delimiter)
                    break
                else:
                    end += 1
            result.append(("string", text[index:end]))
            index = end
            continue
        match = IRI_TOKEN.match(text, index)
        if match:
            result.append(("iri", match.group()))
            index = match.end()
            continue
        match = VARIABLE_TOKEN.match(text, index)
        if match:
            result.append(("variable", match.group()))
            index = match.end()
            continue
        match = WORD_TOKEN.match(text, index)
        if match:
            result.append(("word", match.group()))
            index = match.end()
            continue
        result.append(("punctuation", char))
        index += 1
    return result, "".join(visible)


def _query_prologue(tokens):
    index, prefixes = 0, {}
    while index < len(tokens) and tokens[index][0] == "word":
        word = tokens[index][1].upper()
        if word == "BASE":
            if index + 1 >= len(tokens) or tokens[index + 1][0] != "iri":
                return None, {}
            index += 2
        elif word == "PREFIX":
            if (index + 2 >= len(tokens) or tokens[index + 1][0] != "word"
                    or not tokens[index + 1][1].endswith(":") or tokens[index + 2][0] != "iri"):
                return None, {}
            prefixes[tokens[index + 1][1][:-1]] = _iri_value(tokens[index + 2][1])
            index += 3
        else:
            return index, prefixes
    return None, prefixes


def _query_form(tokens):
    index, _ = _query_prologue(tokens)
    return tokens[index][1].upper() if index is not None else None


def _iri_value(token):
    def unescape(match):
        try:
            return chr(int(match.group()[2:], 16))
        except ValueError:
            return match.group()
    return re.sub(r"\\(?:u[0-9a-fA-F]{4}|U[0-9a-fA-F]{8})", unescape, token[1:-1])


def _query_readiness(text):
    tokens, visible = _tokens(text)
    form = _query_form(tokens)
    # Never put an arbitrary leading word from query text into the inventory.
    form = form if form in SUPPORTED_QUERY_FORMS | UNSUPPORTED_QUERY_FORMS else None
    body_start, prefixes = _query_prologue(tokens)
    body = tokens[body_start:] if body_start is not None else []
    for kind, value in body:
        if kind == "iri" and _iri_value(value).startswith(PARAM_NAMESPACE):
            return form, "QUERY_REQUIRES_PARAMETERS"
        if kind == "word" and ":" in value:
            prefix, local = value.split(":", 1)
            if prefix in prefixes and (prefixes[prefix] + local).startswith(PARAM_NAMESPACE):
                return form, "QUERY_REQUIRES_PARAMETERS"
    if TEMPLATE_SLOT.search(visible):
        return form, "QUERY_REQUIRES_TEMPLATE_EXPANSION"
    if ("variable", "$this") in tokens:
        return form, "QUERY_REQUIRES_SHACL_BINDING"
    for index, token in enumerate(tokens):
        if token != ("word", "VALUES") and not (token[0] == "word" and token[1].upper() == "VALUES"):
            continue
        cursor = index + 1
        if cursor < len(tokens) and tokens[cursor][0] == "variable":
            cursor += 1
        elif cursor < len(tokens) and tokens[cursor][1] == "(":
            cursor += 1
            while cursor < len(tokens) and tokens[cursor][0] == "variable":
                cursor += 1
            if cursor >= len(tokens) or tokens[cursor][1] != ")":
                continue
            cursor += 1
        else:
            continue
        if [value for _, value in tokens[cursor:cursor + 2]] == ["{", "}"]:
            return form, "QUERY_REQUIRES_VALUES"
    if form in SUPPORTED_QUERY_FORMS:
        return form, None
    if form in UNSUPPORTED_QUERY_FORMS:
        return form, "UNSUPPORTED_QUERY_FORM"
    return None, "UNRECOGNIZED_QUERY_FORM"


def _rdf_gap_reason(path):
    if path.suffix.lower() in UNSUPPORTED_RDF_SUFFIXES:
        return "UNSUPPORTED_RDF_FORMAT"
    if any(path.name.lower().endswith(extension + compression)
           for extension in set(RDF_FORMATS) | UNSUPPORTED_RDF_SUFFIXES
           for compression in (".gz", ".bz2", ".xz", ".zst", ".zip")):
        return "COMPRESSED_RDF_UNSUPPORTED"
    return None


def _metadata(path, root, core):
    return {"path": str(path), "relative_path": path.relative_to(root).as_posix(),
            "bytes": path.stat().st_size, "sha256": core.digest_file(path)}


def _read_bytes(path, metadata):
    raw = path.read_bytes()
    if hashlib.sha256(raw).hexdigest() != metadata["sha256"]:
        raise ValueError("repository asset changed during discovery")
    return raw


def _source(metadata):
    return {"path": metadata["path"], "document_id": metadata["relative_path"],
            "format": metadata["format"], "expected_sha256": metadata["sha256"]}


def prepare_repository(repository, generated, work_dir, profile, view, excludes, shapes_root, core):
    """Generate requests; the caller executes them and classifies coverage gaps."""
    repository, generated, work_dir = map(lambda path: Path(os.path.abspath(path)), (repository, generated, work_dir))
    if repository.is_symlink() or not repository.is_dir():
        raise ValueError("ontology-repo must be a directory, not a symlink")
    repository, generated, work_dir = repository.resolve(), generated.resolve(), work_dir.resolve()
    if profile not in PROFILES or view not in {"asserted", "inferred", "closure"}:
        raise ValueError("unsupported repository benchmark profile or view")
    if not all(isinstance(pattern, str) and pattern for pattern in excludes):
        raise ValueError("exclude patterns must be nonempty relative globs")
    if any(Path(pattern).is_absolute() or ".." in Path(pattern).parts for pattern in excludes):
        raise ValueError("exclude patterns must be relative to their scan root")
    generated.mkdir(parents=True, exist_ok=True)
    compile_path, read_dir = generated / "compile.request.json", generated / "read-cases"
    if compile_path.exists() or (read_dir.exists() and any(read_dir.iterdir())):
        raise ValueError("repository request output must be fresh")
    inventory = {"schema_version": SCHEMA, "repository": str(repository),
        "policy": {"scan": "RECURSIVE_REGULAR_FILES_SORTED_BY_RELATIVE_POSIX_PATH",
                   "technical_exclusions": sorted(TECHNICAL_DIRECTORIES), "exclude_globs": list(excludes),
                   "excluded_work_directory": str(work_dir), "excluded_generated_directory": str(generated),
                   "document_id": "REPOSITORY_RELATIVE_POSIX_PATH", "base_iri": "NOT_INFERRED",
                   "profile": profile, "profile_selection": "FIXED_BENCHMARK_POLICY_NOT_PRODUCTION_INFERENCE",
                   "view": view, "imports": "FROZEN_LOCAL_DECLARATIONS_ONLY_NO_NETWORK",
                   "construct": CONSTRUCT_MODE},
        "sources": [], "rules": [], "query_assets": [], "read_cases": [],
        "skipped_assets": [], "excluded_paths": [], "shacl": {}, "counts": {}, "discovery_complete": False}
    core.publish(generated / "inventory.json", inventory)

    def skip(metadata, reason, asset_type):
        inventory["skipped_assets"].append({**metadata, "asset_type": asset_type, "reason_code": reason})

    def emit_read(name, operation, metadata):
        path = read_dir / (name + ".json")
        core.publish(path, {"contract": CONTRACT, "request_id": "repository-" + name, "operation": operation})
        inventory["read_cases"].append({"name": name, "kind": operation["kind"],
                                         "request_file": str(path), **metadata})

    files = _scan(repository, work_dir, generated, excludes, inventory, "repository")
    read_dir.mkdir(parents=True, exist_ok=True)
    ignored_json = 0
    for path in files:
        suffix = path.suffix.lower()
        if suffix in RDF_FORMATS:
            metadata = _metadata(path, repository, core)
            metadata.update(format=RDF_FORMATS[suffix], document_id=metadata["relative_path"])
            inventory["sources"].append(metadata)
        elif suffix == ".json":
            metadata = _metadata(path, repository, core)
            try:
                payload = json.loads(_read_bytes(path, metadata))
            except (UnicodeDecodeError, json.JSONDecodeError):
                skip(metadata, "INVALID_JSON_ASSET", "json")
                continue
            schema = payload.get("schema_version") if isinstance(payload, dict) else None
            if schema == "ontology-rule-ir/v1":
                inventory["rules"].append(metadata)
            elif isinstance(schema, str) and schema.startswith("ontology-rule-ir/"):
                skip(metadata, "UNSUPPORTED_RULE_IR_VERSION", "rule")
            elif isinstance(payload, list) and payload and all(isinstance(rule, dict) and "head" in rule and "body" in rule for rule in payload):
                skip(metadata, "LEGACY_RULE_ARRAY_NOT_ADMITTED", "rule")
            else:
                ignored_json += 1
        elif suffix in {".rq", ".sparql"}:
            metadata = _metadata(path, repository, core)
            try:
                text = _read_bytes(path, metadata).decode("utf8")
            except UnicodeDecodeError:
                skip(metadata, "NON_UTF8_QUERY", "query")
                inventory["query_assets"].append({**metadata, "status": "SKIPPED"})
                continue
            form, reason = _query_readiness(text)
            item = {**metadata, "query_form": form, "status": "SKIPPED" if reason else "GENERATED"}
            inventory["query_assets"].append(item)
            if reason:
                skip(metadata, reason, "query")
                continue
            mode = CONSTRUCT_MODE if form == "CONSTRUCT" else "ORIGINAL_" + form
            name = f"{len(inventory['read_cases']) + 1:04d}-" + ("construct-probe" if form == "CONSTRUCT" else form.lower())
            operation = {"kind": "query", "dataset": str(generated / "reasoned-placeholder"),
                         "query": "ASK {}" if form == "CONSTRUCT" else text, "view": view,
                         "require": ["select-ask/v1", "views/v1"]}
            if form == "CONSTRUCT":
                operation["derived_queries"] = [text]
                operation["require"].append("derived-query-data/v1")
            emit_read(name, operation, {"mode": mode, "asset_path": str(path), "asset_sha256": metadata["sha256"]})
            item.update(mode=mode, request_file=inventory["read_cases"][-1]["request_file"])
        elif _rdf_gap_reason(path):
            skip(_metadata(path, repository, core), _rdf_gap_reason(path), "rdf_candidate")

    core.publish(generated / "inventory.json", inventory)
    external_shapes = []
    if shapes_root is not None:
        shapes_root = Path(os.path.abspath(shapes_root))
        if shapes_root.is_symlink() or not shapes_root.is_dir():
            raise ValueError("shapes-root must be a directory, not a symlink")
        shapes_root = shapes_root.resolve()
        for path in _scan(shapes_root, work_dir, generated, excludes, inventory, "external_shapes"):
            if path.suffix.lower() in RDF_FORMATS:
                metadata = _metadata(path, shapes_root, core)
                metadata.update(format=RDF_FORMATS[path.suffix.lower()], document_id=metadata["relative_path"])
                external_shapes.append(metadata)
            elif _rdf_gap_reason(path):
                skip(_metadata(path, shapes_root, core), _rdf_gap_reason(path), "external_shapes")
        if not external_shapes:
            core.publish(generated / "inventory.json", inventory)
            raise ValueError("shapes-root contains no supported RDF shape files after exclusions")
    mode = "EXTERNAL_SHAPES" if shapes_root is not None else "DATASET_SHAPES"
    inventory["shacl"] = {"mode": mode, "selection": "DECLARED_TARGETS", "shapes_root": str(shapes_root) if shapes_root else None,
                           "sources": external_shapes, "source_count": len(external_shapes),
                           "external_shapes_added_to_compile_sources": False,
                           "known_limitations": ["IMPLICIT_CLASS_TARGETS_UNSUPPORTED", "SHAPES_OWL_IMPORTS_UNSUPPORTED"]}
    if inventory["sources"]:
        emit_read("shacl-" + ("external" if external_shapes else "dataset"),
                  {"kind": "validate", "dataset": str(generated / "reasoned-placeholder"),
                   "shapes": [_source(source) for source in external_shapes],
                   "shapes_from_dataset": not external_shapes}, {"mode": mode})
    inventory["counts"] = {"scanned_files": len(files), "rdf_sources": len(inventory["sources"]),
        "rule_ir_files": len(inventory["rules"]), "query_assets": len(inventory["query_assets"]),
        "query_cases": sum(case["kind"] == "query" for case in inventory["read_cases"]),
        "construct_probes": sum(case.get("mode") == CONSTRUCT_MODE for case in inventory["read_cases"]),
        "shacl_cases": sum(case["kind"] == "validate" for case in inventory["read_cases"]),
        "skipped_assets": len(inventory["skipped_assets"]), "excluded_paths": len(inventory["excluded_paths"]),
        "ignored_json_files": ignored_json}
    inventory["discovery_complete"] = True
    core.publish(generated / "inventory.json", inventory)
    if not inventory["sources"]:
        raise ValueError("ontology-repo contains no supported RDF source files after exclusions")
    core.publish(compile_path, {"contract": CONTRACT, "request_id": "repository-compile", "operation": {
        "kind": "compile", "sources": [_source(source) for source in inventory["sources"]],
        "output": str(generated / "compiled-placeholder"), "profile": profile,
        "rules": [{"path": rule["path"], "expected_sha256": rule["sha256"]} for rule in inventory["rules"]]}})
    return {"compile_request": compile_path, "read_requests": read_dir, "inventory": inventory}
