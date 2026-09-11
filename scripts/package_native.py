"""Build release archives from compiled apps; development tooling only."""
import argparse
import hashlib
from pathlib import Path
import subprocess
import tarfile
import tempfile
import tomllib

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True)
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--output', type=Path, default=Path('dist'))
    options = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    binary = (options.binary or root/'target'/options.target/'release'/'ports').resolve(strict=True)
    options.output.mkdir(parents=True, exist_ok=True)
    version = tomllib.loads((root/'Cargo.toml').read_text())['package']['version']
    if subprocess.check_output([str(binary), '--version'], text=True).strip() != 'ports '+version:
        raise ValueError('Expected Ports '+version)
    archive = options.output / f'ports-{options.target}.tar.gz'
    if archive.exists():
        raise FileExistsError('Use another output directory; release bytes cannot be replaced.')
    with tempfile.TemporaryDirectory(prefix='ports-package-') as name:
        stage = Path(name)
        executable = stage/'ports'
        executable.write_bytes(binary.read_bytes())
        executable.chmod(0o755)
        (stage/'LICENSE.txt').write_bytes((root/'LICENSE').read_bytes())
        (stage/'THIRD_PARTY_NOTICES.txt').write_bytes((root/'docs/licenses/THIRD_PARTY_NOTICES.txt').read_bytes())
        names = ('ports', 'LICENSE.txt', 'THIRD_PARTY_NOTICES.txt')
        (stage/'SHA256SUMS').write_text(''.join(hashlib.sha256((stage/name).read_bytes()).hexdigest()+'  '+name+'\n' for name in names), encoding='ascii')
        with tarfile.open(archive, 'w:gz') as bundle:
            for name in names:
                bundle.add(stage/name, arcname=name)
            bundle.add(stage/'SHA256SUMS', arcname='SHA256SUMS')
    archive.with_name(archive.name+'.sha256').write_text(hashlib.sha256(archive.read_bytes()).hexdigest()+'  '+archive.name+'\n')
    print(archive)

if __name__ == '__main__':
    main()
