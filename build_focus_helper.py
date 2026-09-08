"""Build the small Windows focus executable once for this source revision."""
import hashlib
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent


def ensure_helper():
    source = ROOT / "FocusHelper.cs"
    revision = hashlib.sha256(source.read_bytes()).hexdigest()[:16]
    output = ROOT / "build" / ("FocusHelper-" + revision + ".exe")
    if output.is_file():
        return output
    output.parent.mkdir(parents=True, exist_ok=True)
    framework = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "Microsoft.NET"
    compiler = framework / "Framework64/v4.0.30319/csc.exe"
    if not compiler.is_file():
        compiler = framework / "Framework/v4.0.30319/csc.exe"
    references = []
    for name in ("UIAutomationClient", "UIAutomationTypes", "WindowsBase", "System.Web.Extensions"):
        matches = list((framework / "assembly/GAC_MSIL" / name).glob("*/" + name + ".dll"))
        if not matches:
            raise OSError("Missing .NET Framework assembly: " + name)
        references.append("/reference:" + str(matches[-1]))
    with tempfile.TemporaryDirectory(prefix="focus-build-", dir=output.parent) as directory:
        staged = Path(directory) / "FocusHelper.exe"
        result = subprocess.run([str(compiler), "/nologo", "/noconfig", "/target:exe", "/optimize+",
            "/reference:System.dll", "/reference:System.Core.dll", *references,
            "/out:" + str(staged), str(source)], capture_output=True,
            creationflags=subprocess.CREATE_NO_WINDOW, timeout=30)
        if result.returncode:
            details = (result.stdout + result.stderr).decode(errors="replace").strip()
            raise OSError("Could not compile the Windows focus helper: " + details)
        try:
            os.replace(staged, output)
        except PermissionError:
            # A simultaneous first launch may already have built this revision.
            if not output.is_file():
                raise
    return output


if __name__ == "__main__":
    print(ensure_helper())
