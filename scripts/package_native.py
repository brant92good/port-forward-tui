"""Build release archives from compiled apps; development tooling only."""
import argparse
import hashlib
from pathlib import Path
import subprocess
import tarfile
import tempfile

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True)
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--output', type=Path, default=Path('dist'))
    options = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    binary = (options.binary or root/'target'/options.target/'release'/'ports').resolve(strict=True)
    options.output.mkdir(parents=True, exist_ok=True)
    if subprocess.check_output([str(binary), '--version'], text=True).strip() != 'ports 0.7.0':
        raise ValueError('Expected Ports 0.7.0')
    archive = options.output / f'ports-{options.target}.tar.gz'
    if archive.exists():
        raise FileExistsError('Use another output directory; release bytes cannot be replaced.')
    with tempfile.TemporaryDirectory(prefix='ports-package-') as name:
        stage = Path(name)
        executable = stage/'ports'
        executable.write_bytes(binary.read_bytes())
        executable.chmod(0o755)
        (stage/'SHA256SUMS').write_text(hashlib.sha256(executable.read_bytes()).hexdigest()+'  ports\n')
        with tarfile.open(archive, 'w:gz') as bundle:
            bundle.add(executable, arcname='ports')
            bundle.add(stage/'SHA256SUMS', arcname='SHA256SUMS')
    archive.with_name(archive.name+'.sha256').write_text(hashlib.sha256(archive.read_bytes()).hexdigest()+'  '+archive.name+'\n')
    print(archive)

if __name__ == '__main__':
    main()
