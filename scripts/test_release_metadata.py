import json
from pathlib import Path
import tempfile
import unittest

import release_metadata as release


class ReleaseMetadataTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='ports-release-record-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.dist = self.root/'dist'; self.dist.mkdir()
        self.version = '0.10.0-beta.1'; self.source = 'a'*40
        (self.root/'Cargo.toml').write_text('[package]\nversion="'+self.version+'"\n')
        (self.root/'Cargo.lock').write_text('[[package]]\nname="port-forward-tui"\nversion="'+self.version+'"\n')
        (self.root/'docs/releases').mkdir(parents=True)
        (self.root/'docs/releases'/f'{self.version}.md').write_text('Fixture notes')

    def qualification(self):
        for target in release.TARGETS:
            archive = 'ports-'+target+('.zip' if target.endswith('windows-msvc') else '.tar.gz')
            (self.dist/archive).write_bytes(b'fixture archive '+target.encode())
            (self.dist/(archive+'.sha256')).write_text(release.sha(self.dist/archive)+'  '+archive+'\n')
            release.qualify(self.root, self.dist, self.source, target)

    def test_strict_versions_and_tag(self):
        self.assertEqual(release.identity(self.root, 'v'+self.version)['channel'], 'beta')
        self.assertEqual(release.channel('0.9.1'), 'stable')
        for value in ('latest','v0.10.0-beta.1','0.10.0-beta.0','0.10.0-beta.01','0.10.0-rc.1','01.0.0','0.9.1\n'):
            with self.assertRaises(ValueError):
                release.channel(value)
        with self.assertRaises(ValueError):
            release.identity(self.root, 'v0.9.1')
        (self.root/'Cargo.lock').write_text('[[package]]\nname="port-forward-tui"\nversion="0.9.1"\n')
        with self.assertRaises(ValueError):
            release.identity(self.root)

    def test_full_matrix_record_is_immutable(self):
        self.qualification()
        release.record(self.root, self.dist, self.source, 'v'+self.version)
        record = json.loads((self.dist/'release-record.json').read_text())
        self.assertEqual(len(record['matrix']), 5)
        self.assertFalse(record['automatic_stable_promotion'])
        self.assertEqual(record['stable_default'], '0.9.1')
        self.assertIn('not yet observed', record['post_publication']['status_at_record_creation'])
        before = (self.dist/'release-record.json').read_bytes()
        with self.assertRaises(ValueError):
            release.record(self.root, self.dist, self.source, 'v'+self.version)
        self.assertEqual((self.dist/'release-record.json').read_bytes(), before)

    def test_changed_archive_rejected(self):
        self.qualification()
        next(self.dist.glob('*.zip')).write_bytes(b'changed')
        with self.assertRaises(ValueError):
            release.record(self.root, self.dist, self.source, 'v'+self.version)

    def test_wrong_matrix_source_or_gate_rejected(self):
        self.qualification()
        path = next(self.dist.glob('qualification-*.json'))
        original = json.loads(path.read_text())
        for field, value in (('source_commit','b'*40),('channel','stable'),('checks',[]),('result','fail')):
            modified = dict(original); modified[field] = value
            path.write_text(json.dumps(modified))
            with self.assertRaises(ValueError):
                release.record(self.root, self.dist, self.source, 'v'+self.version)

    def test_missing_target_and_qualification_overwrite_rejected(self):
        self.qualification()
        with self.assertRaises(FileExistsError):
            release.qualify(self.root, self.dist, self.source, release.TARGETS[0])
        next(self.dist.glob('qualification-*.json')).unlink()
        with self.assertRaises(FileNotFoundError):
            release.record(self.root, self.dist, self.source, 'v'+self.version)


if __name__ == '__main__':
    unittest.main()
