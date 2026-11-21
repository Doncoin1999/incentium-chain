"""What this computer can run: memory, GPU and the largest SN0 tier that fits.

Used by the node (incentium_neuron.node) to pick the Qwen3.5 model. The rules are deliberately
simple and err on the safe side; a short speed test after start-up can still move a node down.

  python -m incentium_neuron.hw      # print what was found and the tier it allows
"""
from __future__ import annotations

import json
import os
import platform
import re
import shutil
import subprocess
from dataclasses import asdict, dataclass

GB = 1024**3

# The model files per tier (Qwen3.5, Q4_K_M GGUF). `ram` is the memory the whole computer needs
# when the model runs on the CPU or on Apple's shared memory; `vram` what a separate graphics card
# needs to hold it. Tier 3 (122B-A10B) is for servers and is never picked on a home computer.
CATALOG = {
    0: {"model": "qwen3.5-2b", "size_gb": 1.3, "ram": 4, "vram": 3,
        "url": "https://huggingface.co/bartowski/Qwen_Qwen3.5-2B-GGUF/resolve/main/Qwen_Qwen3.5-2B-Q4_K_M.gguf"},
    1: {"model": "qwen3.5-9b", "size_gb": 5.7, "ram": 16, "vram": 8,
        "url": "https://huggingface.co/bartowski/Qwen_Qwen3.5-9B-GGUF/resolve/main/Qwen_Qwen3.5-9B-Q4_K_M.gguf"},
    2: {"model": "qwen3.5-27b", "size_gb": 16.5, "ram": 32, "vram": 24,
        "url": "https://huggingface.co/bartowski/Qwen_Qwen3.5-27B-GGUF/resolve/main/Qwen_Qwen3.5-27B-Q4_K_M.gguf"},
}


@dataclass
class Hardware:
    os: str               # macos | linux | windows
    arch: str             # arm64 | x64
    cpu: str
    cores: int
    ram_gb: float
    gpu: str              # "Apple M2 (Metal)", "NVIDIA RTX 3060", or ""
    vram_gb: float        # separate graphics memory; 0 for Apple (shared) and for none
    apple_silicon: bool
    disk_free_gb: float

    def max_tier(self) -> int:
        """The largest tier this computer can run well."""
        best = 0
        for tier, m in sorted(CATALOG.items()):
            if self.apple_silicon:
                ok = self.ram_gb >= m["ram"]                       # Metal uses the shared memory
            elif self.vram_gb:
                ok = self.vram_gb >= m["vram"] or (tier == 0 and self.ram_gb >= m["ram"])
            else:
                ok = tier == 0 and self.ram_gb >= m["ram"]         # CPU only: tier 0
            if ok and self.disk_free_gb >= m["size_gb"] + 1:
                best = tier
        return best

    def can_run_at_all(self) -> tuple[bool, str]:
        if self.ram_gb < 3.5:
            return False, f"At least 4 GB of memory is needed (this computer has {self.ram_gb:.0f} GB)."
        if self.disk_free_gb < CATALOG[0]["size_gb"] + 1:
            return False, f"At least {CATALOG[0]['size_gb'] + 1:.0f} GB of free disk is needed."
        return True, ""

    def gpu_layers(self) -> int:
        """llama.cpp -ngl: everything on the GPU when there is one that fits, else none."""
        return 99 if (self.apple_silicon or self.vram_gb) else 0


def _run(cmd: list[str]) -> str:
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=10).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""


def _nvidia() -> tuple[str, float]:
    if not shutil.which("nvidia-smi"):
        return "", 0.0
    out = _run(["nvidia-smi", "--query-gpu=name,memory.total", "--format=csv,noheader,nounits"])
    best = ("", 0.0)
    for line in out.splitlines():
        parts = [p.strip() for p in line.split(",")]
        if len(parts) == 2 and parts[1].replace(".", "").isdigit():
            mem = float(parts[1]) / 1024
            if mem > best[1]:
                best = (parts[0], mem)
    return best


def detect(home: str | None = None) -> Hardware:
    system = platform.system()
    machine = platform.machine().lower()
    arch = "arm64" if machine in ("arm64", "aarch64") else "x64"
    cores = os.cpu_count() or 1
    disk = shutil.disk_usage(home or os.path.expanduser("~")).free / GB
    if system == "Darwin":
        ram = int(_run(["sysctl", "-n", "hw.memsize"]) or 0) / GB
        cpu = _run(["sysctl", "-n", "machdep.cpu.brand_string"]) or "Apple"
        apple = arch == "arm64"
        perf = _run(["sysctl", "-n", "hw.perflevel0.physicalcpu"])
        if perf.isdigit():
            cores = int(perf)                                     # performance cores
        return Hardware("macos", arch, cpu, cores, ram, f"{cpu} (Metal)" if apple else "", 0.0, apple, disk)
    if system == "Windows":
        try:
            import ctypes

            class MS(ctypes.Structure):
                _fields_ = [("l", ctypes.c_ulong), ("load", ctypes.c_ulong), ("total", ctypes.c_ulonglong),
                            ("avail", ctypes.c_ulonglong), ("tp", ctypes.c_ulonglong), ("ap", ctypes.c_ulonglong),
                            ("tv", ctypes.c_ulonglong), ("av", ctypes.c_ulonglong), ("ae", ctypes.c_ulonglong)]
            ms = MS()
            ms.l = ctypes.sizeof(MS)
            ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(ms))
            ram = ms.total / GB
        except Exception:
            ram = 0.0
        gpu, vram = _nvidia()
        return Hardware("windows", arch, platform.processor() or "CPU", cores, ram, gpu, vram, False, disk)
    # Linux
    ram = 0.0
    try:
        m = re.search(r"MemTotal:\s+(\d+)", open("/proc/meminfo").read())
        ram = int(m.group(1)) * 1024 / GB if m else 0.0
    except OSError:
        pass
    cpu = ""
    try:
        m = re.search(r"model name\s*:\s*(.+)", open("/proc/cpuinfo").read())
        cpu = m.group(1).strip() if m else ""
    except OSError:
        pass
    gpu, vram = _nvidia()
    return Hardware("linux", arch, cpu or "CPU", cores, ram, gpu, vram, False, disk)


def choose_tier(hw: Hardware, open_tiers: set[int]) -> int:
    """Largest tier that is both open on chain and within what this computer can run."""
    fit = [t for t in CATALOG if t <= hw.max_tier() and t in open_tiers]
    return max(fit) if fit else 0


if __name__ == "__main__":
    h = detect()
    print(json.dumps({**asdict(h), "max_tier": h.max_tier(), "model": CATALOG[h.max_tier()]["model"]}, indent=2))
