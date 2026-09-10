"""Pure local regressions for notice boundaries; no Cargo/network invocation."""
import tempfile
import hashlib
import json
from pathlib import Path
import unittest
from collect_licenses import dependencies, license_files, fallback_files


class NoticesTests(unittest.TestCase):
    def test_walks_runtime_and_build_edges_but_not_dev_only_subtrees(self):
        edge = lambda package, *kinds: {'pkg': package, 'dep_kinds': [{'kind': kind} for kind in kinds]}
        metadata = {'packages': [{'id': name} for name in ('root', 'runtime', 'build', 'dev', 'dev-child')],
                    'resolve': {'root': 'root', 'nodes': [
                        {'id': 'root', 'deps': [edge('runtime', None, 'dev'), edge('dev', 'dev')]},
                        {'id': 'runtime', 'deps': [edge('build', 'build')]},
                        {'id': 'build', 'deps': []},
                        {'id': 'dev', 'deps': [edge('dev-child', None)]},
                        {'id': 'dev-child', 'deps': []}]}}
        self.assertEqual(set(dependencies(metadata)), {'runtime', 'build'})

    def test_declared_license_file_is_included_and_outside_paths_are_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / 'crate'
            root.mkdir()
            (root / 'Cargo.toml').write_text('fixture', encoding='utf-8')
            (root / 'terms.txt').write_text('Fixture notice', encoding='utf-8')
            package = {'name': 'fixture', 'version': '1.0.0', 'manifest_path': str(root / 'Cargo.toml'), 'license_file': 'terms.txt'}
            self.assertEqual(license_files(package), [('terms.txt', 'Fixture notice')])
            (root.parent / 'private.txt').write_text('must not collect', encoding='utf-8')
            package['license_file'] = '../private.txt'
            with self.assertRaisesRegex(ValueError, 'leaves crate'):
                license_files(package)

    def test_missing_notices_fail_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            manifest = Path(temporary) / 'Cargo.toml'
            manifest.write_text('fixture', encoding='utf-8')
            with self.assertRaisesRegex(ValueError, 'missing notices'):
                license_files({'name': 'fixture', 'version': '1.0.0', 'manifest_path': str(manifest)})

    def test_fallback_is_bound_to_package_version_source_and_document(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root/'Cargo.toml').write_text('fixture', encoding='utf-8')
            (root/'.cargo_vcs_info.json').write_text(json.dumps({'git': {'sha1': 'source'}}), encoding='utf-8')
            (root/'MIT.txt').write_text('Notice', encoding='utf-8')
            index = {'documents': {'MIT.txt': {'url': 'https://example.invalid/pinned', 'sha256': hashlib.sha256(b'Notice').hexdigest()}},
                     'fallbacks': [{'name': 'fixture', 'version': '1.0.0', 'revision': 'source', 'reason': 'Test', 'files': ['MIT.txt']}]}
            (root/'sources.json').write_text(json.dumps(index), encoding='utf-8')
            package = {'name': 'fixture', 'version': '1.0.0', 'manifest_path': str(root/'Cargo.toml')}
            self.assertEqual(len(fallback_files(package, root)), 2)
            package['version'] = '2.0.0'
            with self.assertRaisesRegex(ValueError, 'missing notices'):
                fallback_files(package, root)
            package['version'] = '1.0.0'
            (root/'MIT.txt').write_text('Altered', encoding='utf-8')
            with self.assertRaisesRegex(ValueError, 'digest changed'):
                fallback_files(package, root)
            (root/'.cargo_vcs_info.json').write_text(json.dumps({'git': {'sha1': 'different'}}), encoding='utf-8')
            with self.assertRaisesRegex(ValueError, 'source revision'):
                fallback_files(package, root)


if __name__ == '__main__':
    unittest.main()
