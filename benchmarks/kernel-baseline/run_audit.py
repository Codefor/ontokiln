#!/usr/bin/env python3
"""Post-timing read-only audit of six retained Full artifact identities.

Preparation alone must not run this program. Execute only after all benchmark
processes have exited. It compiles the C reader, hashes databases before/after,
and executes direct SQL; none of this belongs to benchmark timing.
"""
import argparse
from collections import Counter
import datetime
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import time


BASE = Path('/workspace/scratch/002e4172a133')
HERE = Path(__file__).resolve().parent
REPOSITORY = BASE / 'ontokiln'
COMMIT = '46e0fb5'
EXPECTED_BINARY_SHA256 = '1fb31975bc3f81980e5c89650364c1fad56d6b5f75b59dc5bef21c900faefc0c'
PRIOR_PROBE = BASE / 'ontokiln-sqlite-explain-46e0fb5/sqlite-explain-evidence.json'


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def write_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + '.tmp')
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False, allow_nan=False) + '\n', encoding='utf8')
    temporary.replace(path)


def command(args, timeout=60, **options):
    return subprocess.run([str(arg) for arg in args], capture_output=True, text=True,
                          check=True, timeout=timeout, **options)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def freeze_oracle_source():
    revision = command(['git', 'rev-parse', COMMIT + '^{commit}'], cwd=REPOSITORY).stdout.strip()
    source = subprocess.run(['git', 'show', revision + ':benchmarks/kernel_baseline.py'],
                            cwd=REPOSITORY, capture_output=True, check=True).stdout
    blob = command(['git', 'rev-parse', revision + ':benchmarks/kernel_baseline.py'], cwd=REPOSITORY).stdout.strip()
    destination = HERE / 'kernel_baseline_frozen_46e0fb5.py'
    destination.write_bytes(source)
    spec = importlib.util.spec_from_file_location('frozen_kernel_baseline_46e0fb5', destination)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    require(module.GENERATOR == 'kernel-baseline-orders/v1', 'unexpected frozen generator')
    require(module.CHAIN == 8, 'unexpected frozen chain length')
    return module, {'commit': revision, 'repository': str(REPOSITORY), 'path_in_commit': 'benchmarks/kernel_baseline.py',
                    'git_blob': blob, 'source_sha256': hashlib.sha256(source).hexdigest(), 'frozen_copy': str(destination),
                    'expected_values_origin': ['schema_triples', 'record_triples', 'inferred_counts'],
                    'sample_closure_origin': 'independent closed-form RDFS and bounded-chain rules in run_audit.py'}


def build_probe():
    evidence = json.loads(PRIOR_PROBE.read_bytes())['probe']
    library, header = Path(evidence['static_library']), Path(evidence['matching_header'])
    require(sha256(library) == evidence['static_library_sha256'], 'release SQLite static library changed')
    require(sha256(header) == evidence['matching_header_sha256'], 'matching SQLite header changed')
    source, binary = HERE / 'sqlite_audit.c', HERE / 'sqlite_audit'
    args = ['/usr/bin/cc', '-std=c11', '-O2', '-Wall', '-Wextra', '-Werror', source,
            '-I' + str(header.parent), library, '-ldl', '-lpthread', '-lm', '-o', binary]
    result = command(args)
    return binary, {'source': str(source), 'source_sha256': sha256(source),
                    'executable': str(binary), 'executable_sha256': sha256(binary),
                    'compile_command': [str(x) for x in args], 'compile_stdout': result.stdout, 'compile_stderr': result.stderr,
                    'compiler_version': command(['/usr/bin/cc', '--version']).stdout.splitlines()[0],
                    'static_library': str(library), 'static_library_sha256': sha256(library),
                    'matching_header': str(header), 'matching_header_sha256': sha256(header),
                    'engine_link_evidence': evidence['engine_link_evidence'],
                    'prior_probe_evidence': str(PRIOR_PROBE), 'prior_probe_evidence_sha256': sha256(PRIOR_PROBE),
                    'ldd': command(['ldd', binary]).stdout}


def expected_counts(module, facts, kind):
    n = module.records_for(facts)
    asserted = Counter(p for _, p, _ in module.schema_triples())
    per_record = Counter(p for _, p, _ in module.record_triples(0, n))
    asserted.update({p: count * n for p, count in per_record.items()})
    require(sum(asserted.values()) == facts, 'independent explicit count formula mismatch')
    inferred = ({'<' + predicate + '>': count for predicate, count in module.inferred_counts(n).items()}
                if kind == 'reasoned' else {})
    groups = [{'explicit': flag, 'predicate_n': p, 'facts': count}
              for flag, counts in [(0, inferred), (1, asserted)] for p, count in sorted(counts.items())]
    return {'records': n, 'fact_totals': {'total_facts': facts + sum(inferred.values()), 'explicit_facts': facts,
                                       'inferred_facts': sum(inferred.values()), 'invalid_explicit_flags': 0,
                                       'nondefault_graph_facts': 0}, 'predicate_groups': groups}


def expected_samples(module, facts, kind):
    n = module.records_for(facts)
    indices = sorted({i for i in [0, 1, 7, 8, n // 2, n - 2, n - 1] if 0 <= i < n})
    triples = {(0, 1, s, p, o) for s, p, o in module.schema_triples()}
    for i in indices:
        triples.update((0, 1, s, p, o) for s, p, o in module.record_triples(i, n))
    if kind == 'reasoned':
        rdf_type = '<' + module.RDF + 'type>'
        triples.add((0, 0, module.iri('Order'), '<' + module.RDFS + 'subClassOf>', module.iri('Resource')))
        triples.add((0, 0, module.iri('region'), '<' + module.RDFS + 'subPropertyOf>', module.iri('relatedTo')))
        for i in indices:
            subject, customer, region = module.order(i), module.customer(i), module.iri('region/' + str(i % 8))
            triples.add((0, 0, subject, rdf_type, module.iri('CommerceRecord')))
            triples.add((0, 0, subject, rdf_type, module.iri('Resource')))
            triples.add((0, 0, subject, module.iri('inRegion'), region))
            triples.add((0, 0, customer, module.iri('locatedIn'), region))
            triples.add((0, 0, customer, module.iri('relatedTo'), region))
            # Positive reachability inside one bounded chain, including the end
            # node's explicit self-loop. This is independent of any engine output.
            end = min((i // 8 + 1) * 8, n) - 1
            triples.update((0, 0, subject, module.iri('reach'), module.order(j))
                           for j in range(min(i + 1, end), end + 1))
    subjects = sorted({row[2] for row in triples})
    return {'record_indices': indices, 'subject_parameters': subjects,
            'columns': ['g', 'explicit', 'subject_n', 'predicate_n', 'object_n'],
            'rows': [list(row) for row in sorted(triples, key=lambda r: (r[2], r[3], r[4], r[1], r[0]))],
            'comparison': 'exact outgoing relation multiset for these subjects; lexical N-Triples terms, graph and explicit flag'}


def select_artifacts(path, label, sizes, expected_status):
    raw = Path(path).read_bytes()
    report = json.loads(raw)
    require(report.get('status') == expected_status, f'{label}: report is not the expected terminal status')
    require(report['environment']['binary_sha256'] == EXPECTED_BINARY_SHA256, f'{label}: unexpected engine binary')
    reference = {'label': label, 'path': str(Path(path).resolve()), 'sha256': hashlib.sha256(raw).hexdigest(),
                 'status': report['status'], 'binary_sha256': report['environment']['binary_sha256'],
                 'kernel_identity': report['environment']['kernel_identity'],
                 'configuration': report.get('configuration'), 'limits': report.get('limits')}
    selected = []
    for facts in sizes:
        scales = [s for s in report['scales'] if s['facts'] == facts]
        require(len(scales) == 1, f'{label}: missing or duplicate scale {facts}')
        scale = scales[0]
        for name, kind in [('full_compile', 'compiled'), ('full_materialize', 'reasoned')]:
            stages = [s for s in scale['stages'] if s['name'] == name]
            require(len(stages) == 1, f'{label}: missing or duplicate stage {name}')
            candidates = [s for s in stages[0]['samples'] if s.get('sample_index') == 0 and not s.get('warmup')
                          and s.get('status') == 'COMPLETE' and s.get('oracle', {}).get('checked') is True]
            require(len(candidates) == 1, f'{label}: retained sample 0 was not oracle-checked COMPLETE')
            sample = candidates[0]
            artifact = sample['oracle']['artifact']
            directory = Path(artifact['path'])
            require(directory.is_dir(), f'{label}: retained artifact is unavailable: {directory}')
            selected.append({'audit_id': f'{label}-{facts}-{kind}', 'source_report': reference,
                             'facts': facts, 'kind': kind, 'stage': name, 'sample_index': sample['sample_index'],
                             'input_metadata': scale['input'], 'artifact_record': artifact,
                             'artifact_directory': str(directory), 'database': str(directory / 'facts.sqlite')})
    return selected


def run_probe(binary, database, mode, parameters, timeout):
    args = [binary, database, mode, *parameters]
    temporary = HERE / 'temporary-sqlite-files'
    temporary.mkdir(exist_ok=True)
    env = {**os.environ, 'TMPDIR': str(temporary), 'SQLITE_TMPDIR': str(temporary)}
    started = time.monotonic()
    process = subprocess.run([str(x) for x in args], capture_output=True, text=True, timeout=timeout, env=env)
    record = {'command': [str(x) for x in args], 'returncode': process.returncode,
              'stderr': process.stderr, 'post_timing_audit_elapsed_seconds': time.monotonic() - started,
              'temporary_directory': str(temporary)}
    if process.returncode != 0:
        record['stdout'] = process.stdout
        raise ProbeFailure('read-only SQLite probe failed', record)
    try:
        observed = json.loads(process.stdout)
    except ValueError:
        record['stdout'] = process.stdout
        raise ProbeFailure('SQLite probe did not return complete JSON', record)
    require(observed['database_readonly'] is True and observed['read_transaction'] is True,
            'probe did not enforce read-only transaction')
    require(all(q['statement_readonly'] is True for q in observed['queries']), 'probe returned a writable statement')
    record['observed'] = observed
    return record


class ProbeFailure(Exception):
    def __init__(self, message, record):
        super().__init__(message)
        self.record = record


def assert_observations(groups_probe, sample_probe, expected, samples):
    queries = {q['name']: q for q in groups_probe['observed']['queries']}
    totals = queries['fact_totals']
    require(len(totals['rows']) == 1, 'total query must return exactly one row')
    actual_totals = dict(zip(totals['columns'], totals['rows'][0]))
    actual_groups = []
    for explicit, predicate_id, facts, predicate, kind in queries['predicate_groups']['rows']:
        require(type(explicit) is int and explicit in [0, 1], 'invalid explicit flag in predicate group')
        require(type(predicate_id) is int and predicate_id > 0 and kind == 'I' and isinstance(predicate, str),
                'predicate group has a missing or invalid dictionary reference')
        actual_groups.append({'explicit': explicit, 'predicate_n': predicate, 'facts': facts})
    actual_groups.sort(key=lambda row: (row['explicit'], row['predicate_n']))
    relations = sample_probe['observed']['queries'][0]
    require(relations['name'] == 'sample_outgoing' and relations['columns'] == samples['columns'], 'unexpected sample columns')
    actual_rows, expected_rows = Counter(map(tuple, relations['rows'])), Counter(map(tuple, samples['rows']))
    missing = expected_rows - actual_rows
    extra = actual_rows - expected_rows
    assertions = {'totals_match': actual_totals == expected['fact_totals'],
                  'predicate_groups_match': actual_groups == expected['predicate_groups'],
                  'grouped_total_matches_count': sum(row['facts'] for row in actual_groups) == actual_totals['total_facts'],
                  'sample_outgoing_multisets_match': not missing and not extra,
                  'observed_totals': actual_totals, 'observed_predicate_groups': actual_groups,
                  'observed_sample_rows': len(relations['rows']), 'expected_sample_rows': len(samples['rows']),
                  'missing_sample_rows': [{'row': list(row), 'multiplicity': count} for row, count in sorted(missing.items())],
                  'extra_sample_rows': [{'row': list(row), 'multiplicity': count} for row, count in sorted(extra.items())]}
    assertions['pass'] = all(assertions[key] for key in ['totals_match', 'predicate_groups_match',
                                                       'grouped_total_matches_count', 'sample_outgoing_multisets_match'])
    return assertions


def audit_one(entry, module, binary, cache, timeout):
    result = {**entry, 'status': 'RUNNING', 'started_at_utc': now()}
    database = Path(entry['database'])
    manifest_path = Path(entry['artifact_directory']) / 'manifest.json'
    expected = expected_counts(module, entry['facts'], entry['kind'])
    samples = expected_samples(module, entry['facts'], entry['kind'])
    result.update(expected=expected, deterministic_sample_expectation=samples)
    before = None
    try:
        require(entry['input_metadata']['generator'] == module.GENERATOR, 'report generator differs from frozen oracle')
        require(entry['input_metadata']['records'] == expected['records'], 'report input record count differs')
        manifest_bytes = manifest_path.read_bytes()
        manifest_sha = hashlib.sha256(manifest_bytes).hexdigest()
        require(manifest_sha == entry['artifact_record']['manifest_sha256'], 'artifact manifest differs from timed result')
        manifest = json.loads(manifest_bytes)
        require(manifest['kind'] == entry['kind'] and manifest['profile_id'] == module.PROFILE,
                'artifact kind/profile differs from expected workload')
        require(manifest['completion'] == 'COMPLETE', 'artifact has no COMPLETE manifest')
        if entry['kind'] == 'reasoned':
            require(manifest.get('fixpoint_reached') is True, 'reasoned manifest does not declare a fixed point')
        result['manifest_identity'] = {'sha256': manifest_sha, 'kind': manifest['kind'],
                                       'profile_id': manifest['profile_id'], 'fixpoint_reached': manifest.get('fixpoint_reached'),
                                       'content_digest': manifest['content_digest'],
                                       'source_tree_digest': manifest['source_tree_digest'],
                                       'semantic_fingerprint': manifest['semantic_fingerprint'],
                                       'expected_database_sha256': manifest['files']['facts.sqlite']['sha256']}
        before = sha256(database)
        result['database_sha256_before'] = before
        result['database_bytes'] = database.stat().st_size
        require(before == manifest['files']['facts.sqlite']['sha256'], 'database does not match manifest digest')
        require(result['database_bytes'] == manifest['files']['facts.sqlite']['bytes'], 'database size differs from manifest')
        cache_key = (before, entry['facts'], entry['kind'])
        if cache_key in cache:
            prior = cache[cache_key]
            result['query_results_reused_from_audit_id'] = prior['audit_id']
            result['assertions'] = prior['assertions']
            result['reuse_reason'] = 'identical database SHA-256, facts scale, artifact kind and frozen oracle; identity and before/after hashes are still checked independently'
        else:
            result['groups_probe'] = run_probe(binary, database, 'groups', [], timeout)
            result['samples_probe'] = run_probe(binary, database, 'samples', samples['subject_parameters'], timeout)
            result['assertions'] = assert_observations(result['groups_probe'], result['samples_probe'], expected, samples)
        require(result['assertions']['pass'], 'direct SQL counts or deterministic sampled relations differ from independent expectations')
        result['status'] = 'PASS'
    except Exception as error:
        result['status'] = 'FAILED'
        result['error'] = {'type': type(error).__name__, 'message': str(error)}
        if isinstance(error, ProbeFailure):
            result['failed_probe'] = error.record
    finally:
        if before is not None:
            try:
                result['database_sha256_after'] = sha256(database)
                result['database_unchanged'] = result['database_sha256_after'] == before
                result['manifest_sha256_after'] = sha256(manifest_path)
                result['manifest_unchanged'] = result['manifest_sha256_after'] == result['manifest_identity']['sha256']
                if not result['database_unchanged'] or not result['manifest_unchanged']:
                    result['status'] = 'FAILED'
                    result['identity_error'] = 'artifact changed during direct SQL audit'
            except Exception as error:
                result['status'] = 'FAILED'
                result['identity_error'] = str(error)
        result['finished_at_utc'] = now()
    if result['status'] == 'PASS':
        cache[(before, entry['facts'], entry['kind'])] = result
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--scaling-report', type=Path, default=BASE / 'ontokiln-kernel-scaling-46e0fb5-linux-complete.json')
    parser.add_argument('--baseline-report', type=Path, default=BASE / 'ontokiln-kernel-baseline-46e0fb5-linux.json')
    parser.add_argument('--output', type=Path, default=HERE / 'post-timing-direct-sql-audit.json')
    parser.add_argument('--query-timeout-seconds', type=float, default=600)
    args = parser.parse_args(argv)
    audit = {'schema_version': 'ontokiln-post-timing-artifact-audit/v1', 'status': 'RUNNING', 'started_at_utc': now(),
             'timing_scope': 'post-timing independent direct SQLite read audit; all compilation, hashing and SQL here are excluded from every benchmark timing',
             'sparql_performance_verified': False, 'overrides_benchmark_status': False,
             'coverage': ['actual stored fact counts', 'explicit/inferred predicate distributions', 'default graph and flag validity',
                          'exact outgoing relation multiset for deterministic sampled subjects', 'unchanged database bytes before/after'],
             'limits_of_claim': ['not a SPARQL-path pass or performance result', 'not verification of every inferred relation',
                                 'not a proof trace, full SHACL/OWL conformance check or billion-fact measurement'],
             'orchestrator_source': str(Path(__file__).resolve()), 'orchestrator_sha256': sha256(__file__), 'artifacts': []}
    try:
        require(args.query_timeout_seconds > 0, 'query timeout must be positive')
        # Terminal reports are checked before compiling or reading any database.
        entries = select_artifacts(args.scaling_report, 'scaling', [1_000_000, 10_000_000], 'COMPLETE')
        entries += select_artifacts(args.baseline_report, 'baseline512m', [1_000_000], 'COMPLETE_WITH_FAILURES')
        require(len(entries) == 6, 'audit must retain exactly six database identities')
        module, audit['oracle_source'] = freeze_oracle_source()
        binary, audit['probe_build'] = build_probe()
        write_json(args.output, audit)
        cache = {}
        for entry in entries:
            print('AUDIT ' + entry['audit_id'], flush=True)
            result = audit_one(entry, module, binary, cache, args.query_timeout_seconds)
            audit['artifacts'].append(result)
            write_json(args.output, audit)
            print(result['status'] + ' ' + entry['audit_id'], flush=True)
        audit['status'] = 'PASS' if all(item['status'] == 'PASS' for item in audit['artifacts']) else 'FAILED'
        audit['unique_database_contents_queried'] = len({(r['database_sha256_before'], r['facts'], r['kind'])
                                                       for r in audit['artifacts'] if 'groups_probe' in r})
    except Exception as error:
        audit['status'] = 'FAILED'
        audit['fatal_error'] = {'type': type(error).__name__, 'message': str(error)}
    finally:
        audit['finished_at_utc'] = now()
        write_json(args.output, audit)
    print(audit['status'] + ': ' + str(args.output), flush=True)
    return 0 if audit['status'] == 'PASS' else 1


if __name__ == '__main__':
    sys.exit(main())
