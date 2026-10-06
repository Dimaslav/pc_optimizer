from __future__ import annotations

import base64
import copy
import csv
import ctypes
import hashlib
import json
import logging
import os
import platform
import re
import shutil
import stat
import string
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass, field, asdict
from datetime import datetime
from logging.handlers import RotatingFileHandler
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Set, Tuple

from PyQt6.QtCore import (
    Qt, QThread, pyqtSignal, QAbstractTableModel, QModelIndex,
    QSortFilterProxyModel, QSettings, QUrl, QPoint, QTimer,
    QSharedMemory, QLockFile,
)
from PyQt6.QtGui import (
    QDesktopServices, QFont, QAction, QKeySequence,
)
from PyQt6.QtWidgets import (
    QApplication, QAbstractItemView, QCheckBox, QComboBox, QFileDialog,
    QGridLayout, QGroupBox, QHeaderView, QHBoxLayout, QLabel, QLineEdit,
    QMainWindow, QMenu, QMessageBox, QProgressBar, QPushButton,
    QSpinBox, QSplitter, QStatusBar, QTabWidget, QTableView, QTableWidget,
    QTableWidgetItem, QTreeWidget, QTreeWidgetItem, QVBoxLayout, QWidget,
    QInputDialog,
)

# ---- Опциональные зависимости ------------------------------------------------
try:
    from send2trash import send2trash as _send2trash
    HAS_SEND2TRASH = True
except ImportError:
    HAS_SEND2TRASH = False

try:
    import psutil
except ImportError:
    psutil = None

try:
    import winreg  # noqa: F401
    HAS_WINREG = True
except ImportError:
    winreg = None
    HAS_WINREG = False


# ============================================================================
#  КОНСТАНТЫ
# ============================================================================
APP_NAME = "PC Optimizer + Disk Scanner"
APP_VERSION = "1.1.0"
ORG_NAME = "DiskTools"

IS_WINDOWS = platform.system() == "Windows"
IS_LINUX = platform.system() == "Linux"
IS_MAC = platform.system() == "Darwin"

APP_DATA_DIR = Path.home() / ".pc_optimizer"
CONFIG_DIR = APP_DATA_DIR / "config"
LOG_DIR = APP_DATA_DIR / "logs"
REPORT_DIR = APP_DATA_DIR / "reports"
STARTUP_BACKUP_DIR = APP_DATA_DIR / "startup_backups"

for _d in (APP_DATA_DIR, CONFIG_DIR, LOG_DIR, REPORT_DIR, STARTUP_BACKUP_DIR):
    _d.mkdir(parents=True, exist_ok=True)

CONFIG_FILE = CONFIG_DIR / "settings.json"
STARTUP_BACKUP_FILE = CONFIG_DIR / "startup_backups.json"
LOG_FILE = LOG_DIR / "pc_optimizer.log"

# --- Категории файлов (для сканера диска) -----------------------------------
CATEGORIES: Dict[str, set] = {
    "Изображения": {".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp", ".tiff",
                     ".tif", ".svg", ".heic", ".raw", ".cr2", ".nef", ".orf", ".arw"},
    "Видео": {".mp4", ".mkv", ".avi", ".mov", ".wmv", ".flv", ".webm",
              ".m4v", ".mpeg", ".mpg", ".3gp", ".mts", ".m2ts"},
    "Аудио": {".mp3", ".wav", ".flac", ".aac", ".ogg", ".m4a", ".wma", ".ape"},
    "Документы": {".pdf", ".doc", ".docx", ".xls", ".xlsx", ".ppt", ".pptx",
                  ".txt", ".rtf", ".md", ".odt", ".ods", ".odp", ".csv"},
    "Архивы": {".zip", ".rar", ".7z", ".tar", ".gz", ".bz2", ".xz", ".zst"},
    "Код": {".py", ".js", ".ts", ".html", ".css", ".java", ".c", ".cpp",
            ".h", ".hpp", ".go", ".rs", ".php", ".rb", ".sh", ".bat",
            ".ps1", ".json", ".yaml", ".yml", ".xml", ".sql"},
    "Приложения": {".exe", ".msi", ".apk", ".dmg", ".pkg", ".deb", ".rpm",
                   ".appimage", ".jar"},
    "Образы дисков": {".iso", ".img", ".vhd", ".vhdx", ".nrg", ".bin"},
}
OTHER_CATEGORY = "Прочее"
EXT_TO_CATEGORY: Dict[str, str] = {
    ext: cat for cat, exts in CATEGORIES.items() for ext in exts
}
ALL_CATEGORY_NAMES: List[str] = sorted(CATEGORIES.keys()) + [OTHER_CATEGORY]

CATEGORY_NAMES_RU = {
    "temp": "Временные файлы",
    "thumbnails": "Кэш миниатюр",
    "privacy": "Кэши браузеров",
    "games": "Игровые кэши",
    "recent": "Недавние документы",
}

_PROGRESS_FILE_INTERVAL = 200
_PROGRESS_TIME_INTERVAL = 0.2

STARTUP_RUN_PATH = r"Software\Microsoft\Windows\CurrentVersion\Run"

# Критические процессы, которые нельзя завершать
CRITICAL_PROCESS_NAMES = {
    "system", "system idle process", "registry", "memory compression",
    "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe",
    "lsass.exe", "svchost.exe", "explorer.exe", "dwm.exe", "fontdrvhost.exe",
    "sihost.exe", "taskhostw.exe", "ctfmon.exe", "audiodg.exe",
    "init", "systemd", "kthreadd", "kworker", "ksoftirqd", "migration",
    "watchdog", "launchd", "kernel_task",
}


# ============================================================================
#  ЛОГИРОВАНИЕ
# ============================================================================
_LOG_HANDLER = RotatingFileHandler(
    LOG_FILE, maxBytes=5 * 1024 * 1024, backupCount=3, encoding="utf-8",
)
logging.basicConfig(
    handlers=[_LOG_HANDLER],
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
)
log = logging.getLogger(__name__)


# ============================================================================
#  УТИЛИТЫ
# ============================================================================
def human_size(num: float) -> str:
    try:
        value = float(num or 0)
    except (TypeError, ValueError):
        value = 0.0
    if value < 0:
        return "—"
    units = ["B", "KB", "MB", "GB", "TB", "PB"]
    for i, unit in enumerate(units):
        if value < 1024 or i == len(units) - 1:
            return f"{int(value)} {unit}" if unit == "B" else f"{value:.1f} {unit}"
        value /= 1024
    return f"{value:.1f} PB"


def format_dt(ts: float) -> str:
    try:
        return datetime.fromtimestamp(ts).strftime("%Y-%m-%d %H:%M")
    except (OSError, OverflowError, ValueError):
        return "—"


def normalize_path(path: str) -> str:
    if not path:
        return ""
    path = str(path).strip().strip('"')
    path = os.path.expandvars(os.path.expanduser(path))
    try:
        p = os.path.normcase(os.path.normpath(os.path.abspath(path)))
    except (OSError, ValueError):
        return ""
    # Убираем trailing separator, кроме корня диска
    if len(p) > 1 and p.endswith(os.sep) and not (len(p) == 3 and p[1] == ":"):
        p = p.rstrip(os.sep)
    return p


def normalize_paths(paths: List[str]) -> List[str]:
    seen, result = set(), []
    for p in paths:
        n = normalize_path(p)
        if not n or n in seen:
            continue
        seen.add(n)
        result.append(n)
    return result


def classify_file(path: str) -> str:
    return EXT_TO_CATEGORY.get(Path(path).suffix.lower(), OTHER_CATEGORY)


def is_inside(path: str, root: str, allow_equal: bool = False) -> bool:
    path, root = normalize_path(path), normalize_path(root)
    if not path or not root:
        return False
    if path == root:
        return allow_equal
    try:
        common = os.path.commonpath([path, root])
    except ValueError:
        return False
    return common == root


def path_depth(display: str, root: str) -> int:
    try:
        rel = os.path.relpath(display, root)
        return 0 if rel == os.curdir else rel.count(os.sep) + 1
    except ValueError:
        return 0


def folder_label(display: str, root: str) -> str:
    if normalize_path(display) == normalize_path(root):
        return display
    return os.path.basename(os.path.normpath(display)) or display


def safe_filename(value: str, max_length: int = 100) -> str:
    value = re.sub(r'[<>:"/\\|?*\x00-\x1f]+', "_", str(value)).strip(" ._")
    return (value or "item")[:max_length]


def get_user_profile() -> str:
    return normalize_path(os.environ.get("USERPROFILE") or os.path.expanduser("~"))


def get_system_drive() -> str:
    if IS_WINDOWS:
        return normalize_path(os.environ.get("SystemDrive", "C:") + "\\")
    return "/"


def get_desktop() -> str:
    d = os.path.join(get_user_profile(), "Desktop")
    return d if os.path.isdir(d) else get_user_profile()


def open_in_explorer(path: str) -> None:
    try:
        if IS_WINDOWS:
            if os.path.isfile(path):
                # Explorer ожидает /select,"C:\path" одним аргументом
                subprocess.run(["explorer", f"/select,{os.path.normpath(path)}"],
                               check=False)
            else:
                os.startfile(path)  # noqa
        elif IS_MAC:
            if os.path.isfile(path):
                subprocess.Popen(["open", "-R", path])
            else:
                subprocess.Popen(["open", path])
        else:
            folder = path if os.path.isdir(path) else os.path.dirname(path)
            subprocess.Popen(["xdg-open", folder])
    except Exception:
        log.exception("open_in_explorer failed for %r", path)
        folder = path if os.path.isdir(path) else os.path.dirname(path)
        QDesktopServices.openUrl(QUrl.fromLocalFile(folder))


def is_drive_root(path: str) -> bool:
    path = normalize_path(path)
    drive, tail = os.path.splitdrive(path)
    return bool(drive) and tail in ("\\", "/")


def is_reparse_point(path: str) -> bool:
    if not os.path.lexists(path):
        return False
    try:
        if os.path.islink(path):
            return True
    except OSError:
        return False
    if not IS_WINDOWS:
        return False
    try:
        attrs = ctypes.windll.kernel32.GetFileAttributesW(str(path))
        if attrs == 0xFFFFFFFF:
            return False
        return bool(attrs & 0x0400)  # FILE_ATTRIBUTE_REPARSE_POINT
    except Exception:
        return False


def entry_is_reparse(entry: os.DirEntry) -> bool:
    """Проверка reparse/junction без лишних системных вызовов на Windows."""
    try:
        if entry.is_symlink():
            return True
    except OSError:
        return True
    if not IS_WINDOWS:
        return False
    try:
        st = entry.stat(follow_symlinks=False)
        attr = getattr(st, "st_file_attributes", 0)
        return bool(attr & 0x0400)
    except OSError:
        return True


def run_command(arguments: List[str], timeout: int = 60) -> Dict[str, Any]:
    try:
        result = subprocess.run(
            arguments, capture_output=True, text=True,
            shell=False, timeout=timeout, errors="replace",
        )
        return {
            "success": result.returncode == 0,
            "returncode": result.returncode,
            "stdout": result.stdout or "",
            "stderr": result.stderr or "",
        }
    except subprocess.TimeoutExpired:
        return {"success": False, "returncode": -1, "stdout": "",
                "stderr": "Превышено время ожидания"}
    except Exception as exc:
        log.exception("Ошибка команды: %r", arguments)
        return {"success": False, "returncode": -1, "stdout": "", "stderr": str(exc)}


# ============================================================================
#  КЭШИ ЗАЩИТЫ
# ============================================================================
_EXT_CACHE: Optional[frozenset] = None
_ALLOW_CACHE: Optional[frozenset] = None
_CACHE_LOCK = threading.RLock()


def _invalidate_protection_cache() -> None:
    global _EXT_CACHE, _ALLOW_CACHE
    with _CACHE_LOCK:
        _EXT_CACHE = None
        _ALLOW_CACHE = None


# ============================================================================
#  НАСТРОЙКИ (потокобезопасные)
# ============================================================================
DEFAULT_SETTINGS: Dict[str, Any] = {
    "appearance": "dark",
    "min_age_hours": 72,
    "big_file_mb": 250,
    "duplicate_mb": 20,
    "keep_extensions": [".sys", ".dll", ".drv", ".ocx", ".msi", ".msp",
                        ".efi", ".winmd", ".manifest"],
    "custom_protected": [],
    "safe_mode": True,
    "last_cleanup": "",
    "last_freed": 0,
    "geometry": None,
    "window_state": None,
    "last_scan_path": "",
    "ignore_dirs": ["$recycle.bin", "system volume information", "windowsapps"],
}

_LIST_OF_STR_KEYS = {"keep_extensions", "custom_protected", "ignore_dirs"}


def _normalize_extension(ext: str) -> str:
    ext = str(ext).strip().lower()
    if not ext:
        return ""
    if not ext.startswith("."):
        ext = "." + ext
    return ext


class Settings:
    """Потокобезопасное хранилище настроек."""

    def __init__(self) -> None:
        self._lock = threading.RLock()
        self.data: Dict[str, Any] = copy.deepcopy(DEFAULT_SETTINGS)
        self.load()

    # ------------------------------------------------------------- load/save
    def load(self) -> None:
        loaded: Dict[str, Any] = {}
        try:
            if CONFIG_FILE.exists():
                with open(CONFIG_FILE, "r", encoding="utf-8") as fh:
                    loaded = json.load(fh)
                if not isinstance(loaded, dict):
                    loaded = {}
        except Exception:
            log.exception("Не удалось загрузить настройки")
            loaded = {}

        with self._lock:
            for k, default_v in DEFAULT_SETTINGS.items():
                v = loaded.get(k, default_v)
                v = self._coerce(k, v, default_v)
                self.data[k] = v
        _invalidate_protection_cache()

    @staticmethod
    def _coerce(key: str, value: Any, default: Any) -> Any:
        try:
            if key in _LIST_OF_STR_KEYS:
                if not isinstance(value, list):
                    return list(default)
                if key == "keep_extensions":
                    normalized = [_normalize_extension(x) for x in value]
                    normalized = [x for x in normalized if x]
                    return normalized or list(default)  # запрет пустого списка
                return [str(x) for x in value if str(x).strip()]
            if isinstance(default, bool):
                return bool(value)
            if isinstance(default, int) and not isinstance(default, bool):
                return int(value)
            if isinstance(default, float):
                return float(value)
            if isinstance(default, str):
                return str(value)
            return value
        except (TypeError, ValueError):
            return copy.deepcopy(default)

    def save(self) -> bool:
        with self._lock:
            snapshot = copy.deepcopy(self.data)
        try:
            tmp = str(CONFIG_FILE) + ".tmp"
            with open(tmp, "w", encoding="utf-8", newline="\n") as fh:
                json.dump(snapshot, fh, ensure_ascii=False, indent=2)
            os.replace(tmp, CONFIG_FILE)
            return True
        except Exception:
            log.exception("Не удалось сохранить настройки")
            return False

    # ------------------------------------------------------------- accessors
    def __getitem__(self, key):
        with self._lock:
            return self.data.get(key, DEFAULT_SETTINGS.get(key))

    def __setitem__(self, key, value):
        with self._lock:
            self.data[key] = value
        if key in _LIST_OF_STR_KEYS:
            _invalidate_protection_cache()

    def snapshot(self) -> Dict[str, Any]:
        with self._lock:
            return copy.deepcopy(self.data)


SETTINGS = Settings()


# ============================================================================
#  ЗАЩИТА СИСТЕМНЫХ ФАЙЛОВ
# ============================================================================
def _get_system_roots() -> Set[str]:
    if IS_WINDOWS:
        win = os.environ.get("SystemRoot", r"C:\Windows")
        return {
            win,
            os.path.join(win, "System32"),
            os.path.join(win, "SysWOW64"),
            os.path.join(win, "WinSxS"),
            os.path.join(win, "Fonts"),
            os.path.join(win, "assembly"),
            os.path.join(win, "Boot"),
            os.path.join(win, "servicing"),
            os.path.join(win, "Microsoft.NET"),
            os.environ.get("ProgramFiles", r"C:\Program Files"),
            os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"),
            os.environ.get("ProgramData", r"C:\ProgramData"),
        }
    if IS_LINUX:
        return {"/bin", "/sbin", "/lib", "/lib64", "/usr", "/etc", "/boot",
                "/proc", "/sys", "/dev", "/opt"}
    if IS_MAC:
        return {"/System", "/Library", "/usr", "/bin", "/sbin",
                "/private/var/db", "/Applications"}
    return set()


SYSTEM_ROOTS = frozenset(normalize_path(p) for p in _get_system_roots() if p)
SYSTEM_ROOTS = frozenset(p for p in SYSTEM_ROOTS if p)

PROTECTED_NAMES = {
    "desktop.ini", "bootmgr", "ntldr", "pagefile.sys",
    "hiberfil.sys", "swapfile.sys", "ntuser.dat", "ntuser.ini",
    "boot.ini", "bcd", "autorun.inf", ".ds_store",
    "fstab", "passwd", "shadow", "hosts", "sudoers",
}


def _get_protected_exts() -> frozenset:
    global _EXT_CACHE
    with _CACHE_LOCK:
        if _EXT_CACHE is None:
            _EXT_CACHE = frozenset(
                _normalize_extension(e) for e in SETTINGS["keep_extensions"]
                if _normalize_extension(e)
            )
        return _EXT_CACHE


def _get_allow_paths() -> frozenset:
    global _ALLOW_CACHE
    with _CACHE_LOCK:
        if _ALLOW_CACHE is not None:
            return _ALLOW_CACHE
        allow: Set[str] = set()
        if IS_WINDOWS:
            win = os.environ.get("SystemRoot", r"C:\Windows")
            for sub in ("Temp", "Prefetch", "Logs", "SoftwareDistribution"):
                p = normalize_path(os.path.join(win, sub))
                if p:
                    allow.add(p)
            allow.add(normalize_path(r"C:\$Recycle.Bin"))
        elif IS_LINUX:
            allow.update({normalize_path(p) for p in ("/tmp", "/var/tmp") if p})
        elif IS_MAC:
            allow.add(normalize_path("/tmp"))
        allow.discard("")
        _ALLOW_CACHE = frozenset(allow)
        return _ALLOW_CACHE


def is_protected(path: str) -> Tuple[bool, str]:
    try:
        ap = normalize_path(path)
    except Exception:
        return True, "невалидный путь"
    if not ap:
        return True, "невалидный путь"

    custom = SETTINGS["custom_protected"]
    for cp in custom:
        try:
            cpn = normalize_path(cp)
        except Exception:
            continue
        if not cpn:
            continue
        if ap == cpn or is_inside(ap, cpn, allow_equal=False):
            return True, "пользовательская защита"

    allow_paths = _get_allow_paths()
    is_allowed = any(ap == a or is_inside(ap, a, allow_equal=False)
                     for a in allow_paths)

    if not is_allowed:
        for root in SYSTEM_ROOTS:
            if ap == root or is_inside(ap, root, allow_equal=False):
                return True, "системная папка"

    name = os.path.basename(ap).lower()
    if name in PROTECTED_NAMES:
        return True, "критический файл"

    ext = os.path.splitext(name)[1].lower()
    if ext and ext in _get_protected_exts():
        return True, f"защищённое расширение {ext}"

    if IS_WINDOWS and not is_allowed:
        try:
            attrs = ctypes.windll.kernel32.GetFileAttributesW(str(path))
            if attrs not in (-1, 0xFFFFFFFF) and (attrs & 0x4):
                return True, "атрибут SYSTEM"
        except Exception:
            pass

    try:
        if os.path.islink(path):
            return True, "символическая ссылка"
    except OSError:
        pass

    return False, ""


def validate_delete_path(path: str, root: str) -> Tuple[bool, str]:
    p = normalize_path(path)
    r = normalize_path(root)
    if not p or not r:
        return False, "Пустой путь"
    if is_drive_root(p):
        return False, "Корень диска защищён"
    if p == r:
        return False, "Корень категории защищён"
    if not is_inside(p, r, allow_equal=False):
        return False, "Путь вне разрешённой папки"
    if is_reparse_point(p):
        return False, "Ссылки и reparse points запрещены"
    prot, reason = is_protected(p)
    if prot:
        return False, reason
    return True, ""


# ============================================================================
#  DATACLASS'Ы
# ============================================================================
@dataclass
class FileInfo:
    path: str
    normalized_path: str
    dir_path: str
    name: str
    category: str
    size: int
    mtime: float
    search_blob: str


@dataclass
class FolderStats:
    display: str
    parent_key: Optional[str]
    count: int = 0
    size_bytes: int = 0


@dataclass
class CategoryStats:
    count: int = 0
    size_bytes: int = 0


@dataclass
class ScanResult:
    files: List[FileInfo]
    folder_stats: Dict[str, FolderStats]
    category_stats: Dict[str, CategoryStats]
    total_files: int
    total_bytes: int
    scan_errors: int
    root_display: str


@dataclass
class Candidate:
    category: str
    path: str
    root: str
    size: int
    modified: float


@dataclass
class OperationResult:
    status: str = "success"
    message: str = ""
    deleted: int = 0
    skipped: int = 0
    locked: int = 0
    access_denied: int = 0
    errors: int = 0
    bytes_freed: int = 0
    cancelled: bool = False
    details: List[Dict[str, Any]] = field(default_factory=list)

    def merge(self, other: "OperationResult") -> "OperationResult":
        self.deleted += other.deleted
        self.skipped += other.skipped
        self.locked += other.locked
        self.access_denied += other.access_denied
        self.errors += other.errors
        self.bytes_freed += other.bytes_freed
        self.cancelled = self.cancelled or other.cancelled
        self.details.extend(other.details)
        if self.cancelled:
            self.status = "cancelled"
        elif self.errors or self.locked or self.access_denied:
            self.status = "partial"
        return self


@dataclass
class FolderSize:
    path: str
    name: str
    size: int
    files: int
    is_root_files: bool = False


class CancelToken:
    def __init__(self) -> None:
        self._event = threading.Event()

    def cancel(self) -> None:
        self._event.set()

    def cancelled(self) -> bool:
        return self._event.is_set()


class ScanCanceled(Exception):
    """Выбрасывается при запросе остановки сканирования."""


# ============================================================================
#  СКАНИРОВАНИЕ ДИСКА
# ============================================================================
def scan_disk(
    root_path: str,
    progress_callback: Optional[Callable[[int, int, str], None]] = None,
    stop_requested: Optional[Callable[[], bool]] = None,
) -> ScanResult:
    root_display = os.path.normpath(os.path.abspath(os.path.expanduser(root_path)))
    root_key = normalize_path(root_display)
    if not root_key:
        raise ValueError(f"Невалидный путь: {root_path!r}")

    files: List[FileInfo] = []
    folder_stats: Dict[str, FolderStats] = {
        root_key: FolderStats(display=root_display, parent_key=None)
    }
    category_stats: Dict[str, CategoryStats] = {}
    total_files = 0
    total_bytes = 0
    scan_errors = 0
    last_emit_count = 0
    last_emit_time = time.monotonic()

    def _should_stop() -> bool:
        return bool(stop_requested and stop_requested())

    def _emit(hint: str, force: bool = False) -> None:
        nonlocal last_emit_count, last_emit_time
        if progress_callback is None:
            return
        now = time.monotonic()
        if (force or total_files - last_emit_count >= _PROGRESS_FILE_INTERVAL
                or now - last_emit_time >= _PROGRESS_TIME_INTERVAL):
            progress_callback(total_files, total_bytes, hint)
            last_emit_count = total_files
            last_emit_time = now

    stack: List[Tuple[str, str]] = [(root_display, root_key)]

    while stack:
        if _should_stop():
            raise ScanCanceled()
        dir_display, dir_key = stack.pop()

        try:
            entries = list(os.scandir(dir_display))
        except OSError:
            scan_errors += 1
            continue

        _emit(dir_display)

        for entry in entries:
            if _should_stop():
                raise ScanCanceled()
            try:
                # Пропускаем symlinks и junction/reparse points
                if entry_is_reparse(entry):
                    continue
                if entry.is_dir(follow_symlinks=False):
                    child_display = os.path.normpath(entry.path)
                    child_key = normalize_path(child_display)
                    if not child_key:
                        continue
                    if child_key not in folder_stats:
                        folder_stats[child_key] = FolderStats(
                            display=child_display, parent_key=dir_key
                        )
                        stack.append((child_display, child_key))
                    continue
                if not entry.is_file(follow_symlinks=False):
                    continue
                st = entry.stat(follow_symlinks=False)
            except OSError:
                scan_errors += 1
                continue

            full_path = os.path.normpath(entry.path)
            size = st.st_size
            mtime = st.st_mtime
            category = classify_file(full_path)
            norm = normalize_path(full_path)
            if not norm:
                continue

            files.append(FileInfo(
                path=full_path,
                normalized_path=norm,
                dir_path=dir_display,
                name=entry.name,
                category=category,
                size=size,
                mtime=mtime,
                search_blob=f"{entry.name} {full_path} {category}".lower(),
            ))
            total_files += 1
            total_bytes += size
            folder_stats[dir_key].count += 1
            folder_stats[dir_key].size_bytes += size

            cstats = category_stats.setdefault(category, CategoryStats())
            cstats.count += 1
            cstats.size_bytes += size

            _emit(full_path)

    # Агрегация размеров вверх по дереву
    sorted_keys = sorted(
        folder_stats,
        key=lambda k: path_depth(folder_stats[k].display, root_display),
        reverse=True,
    )
    for key in sorted_keys:
        if key == root_key:
            continue
        stats = folder_stats[key]
        pk = stats.parent_key
        if pk and pk in folder_stats and pk != key:
            folder_stats[pk].count += stats.count
            folder_stats[pk].size_bytes += stats.size_bytes

    _emit(root_display, force=True)
    return ScanResult(
        files=files,
        folder_stats=folder_stats,
        category_stats=category_stats,
        total_files=total_files,
        total_bytes=total_bytes,
        scan_errors=scan_errors,
        root_display=root_display,
    )


# ============================================================================
#  КАТАЛОГИ ОЧИСТКИ
# ============================================================================
def temp_roots() -> List[str]:
    return normalize_paths([
        tempfile.gettempdir(),
        os.environ.get("TEMP", ""),
        os.environ.get("TMP", ""),
    ])


def thumbnail_roots() -> List[str]:
    return normalize_paths([
        os.path.join(get_user_profile(), "AppData", "Local", "Microsoft",
                     "Windows", "Explorer")
    ])


def recent_roots() -> List[str]:
    return normalize_paths([
        os.path.join(get_user_profile(), "AppData", "Roaming", "Microsoft",
                     "Windows", "Recent")
    ])


def browser_cache_roots() -> List[str]:
    user = get_user_profile()
    result: List[str] = []
    bases = [
        os.path.join(user, "AppData", "Local", "Google", "Chrome", "User Data"),
        os.path.join(user, "AppData", "Local", "Microsoft", "Edge", "User Data"),
        os.path.join(user, "AppData", "Local", "BraveSoftware",
                     "Brave-Browser", "User Data"),
    ]
    profile_pattern = re.compile(r"^(Default|Profile \d+|Guest Profile)$", re.I)
    for base in bases:
        if not os.path.isdir(base):
            continue
        try:
            for entry in os.scandir(base):
                if (entry.is_dir(follow_symlinks=False)
                        and not entry_is_reparse(entry)
                        and profile_pattern.match(entry.name)):
                    result.extend([
                        os.path.join(entry.path, "Cache"),
                        os.path.join(entry.path, "Code Cache"),
                        os.path.join(entry.path, "GPUCache"),
                        os.path.join(entry.path, "Service Worker", "CacheStorage"),
                    ])
        except OSError:
            pass

    firefox = os.path.join(user, "AppData", "Local", "Mozilla", "Firefox", "Profiles")
    if os.path.isdir(firefox):
        try:
            for entry in os.scandir(firefox):
                if entry.is_dir(follow_symlinks=False) and not entry_is_reparse(entry):
                    result.extend([
                        os.path.join(entry.path, "cache2"),
                        os.path.join(entry.path, "startupCache"),
                    ])
        except OSError:
            pass

    result.append(os.path.join(user, "AppData", "Local", "Microsoft",
                                "Windows", "INetCache"))
    return normalize_paths(result)


def game_cache_roots() -> List[str]:
    user = get_user_profile()
    return normalize_paths([
        os.path.join(user, "AppData", "Local", "D3DSCache"),
        os.path.join(user, "AppData", "Local", "NVIDIA", "DXCache"),
        os.path.join(user, "AppData", "Local", "NVIDIA", "GLCache"),
        os.path.join(user, "AppData", "Roaming", "discord", "Cache"),
        os.path.join(user, "AppData", "Roaming", "discord", "Code Cache"),
        os.path.join(user, "AppData", "Roaming", "discord", "GPUCache"),
        os.path.join(user, "AppData", "Local", "EpicGamesLauncher", "Saved",
                     "webcache"),
        os.path.join(user, "AppData", "Local", "Battle.net", "Cache"),
    ])


def category_roots(category: str) -> List[str]:
    fn = {
        "temp": temp_roots,
        "thumbnails": thumbnail_roots,
        "privacy": browser_cache_roots,
        "games": game_cache_roots,
        "recent": recent_roots,
    }.get(category)
    return fn() if fn else []


def category_accepts(category: str, path: str) -> bool:
    if category != "thumbnails":
        return True
    name = os.path.basename(path).lower()
    return name.endswith(".db") and (name.startswith("thumbcache")
                                     or name.startswith("iconcache"))


# ============================================================================
#  ОЧИСТКА: СКАНИРОВАНИЕ И УДАЛЕНИЕ
# ============================================================================
def scan_cleanup(categories: List[str], min_age_hours: int,
                 token: CancelToken,
                 progress: Callable[[str], None]) -> List[Candidate]:
    candidates: Dict[str, Candidate] = {}
    cutoff = time.time() - max(0, min_age_hours) * 3600
    scanned = 0

    for category in categories:
        if token.cancelled():
            break
        progress(f"Сканирование: {CATEGORY_NAMES_RU.get(category, category)}")
        for root in category_roots(category):
            if token.cancelled():
                break
            if not os.path.isdir(root) or is_reparse_point(root):
                continue
            stack = [root]
            while stack and not token.cancelled():
                current = stack.pop()
                if current != root and is_reparse_point(current):
                    continue
                try:
                    with os.scandir(current) as it:
                        for entry in it:
                            if token.cancelled():
                                break
                            try:
                                if entry_is_reparse(entry):
                                    continue
                                if entry.is_dir(follow_symlinks=False):
                                    stack.append(entry.path)
                                    continue
                                if not entry.is_file(follow_symlinks=False):
                                    continue
                                scanned += 1
                                if scanned % 300 == 0:
                                    progress(f"Проверено файлов: {scanned}")
                                if not category_accepts(category, entry.path):
                                    continue
                                info = entry.stat(follow_symlinks=False)
                                if min_age_hours > 0 and info.st_mtime > cutoff:
                                    continue
                                valid, _ = validate_delete_path(entry.path, root)
                                if not valid:
                                    continue
                                candidates[normalize_path(entry.path)] = Candidate(
                                    category=category, path=entry.path, root=root,
                                    size=int(info.st_size),
                                    modified=float(info.st_mtime),
                                )
                            except (OSError, PermissionError, FileNotFoundError):
                                continue
                except (OSError, PermissionError, FileNotFoundError):
                    continue
    result = list(candidates.values())
    result.sort(key=lambda c: c.size, reverse=True)
    return result


def _make_writable(path: str) -> None:
    """Корректно снимает read-only на файле, не ломая остальные биты."""
    try:
        st = os.stat(path)
    except OSError:
        return
    mode = st.st_mode
    if mode & stat.S_IWRITE:
        return
    try:
        os.chmod(path, mode | stat.S_IWRITE)
    except OSError:
        pass


def delete_candidates(candidates: List[Candidate], token: CancelToken,
                      progress: Callable[[str], None]) -> OperationResult:
    result = OperationResult()
    total = len(candidates)

    for index, c in enumerate(candidates, 1):
        if token.cancelled():
            result.status = "cancelled"
            result.cancelled = True
            result.message = "Отменено пользователем"
            break
        if index % 20 == 0 or index == total:
            progress(f"Удаление: {index}/{total}")
        valid, reason = validate_delete_path(c.path, c.root)
        if not valid:
            result.skipped += 1
            result.details.append({"path": c.path, "success": False,
                                   "error": reason})
            continue
        try:
            if not os.path.isfile(c.path):
                result.skipped += 1
                continue
            _make_writable(c.path)
            size = os.path.getsize(c.path)
            os.remove(c.path)
            result.deleted += 1
            result.bytes_freed += size
            result.details.append({"path": c.path, "success": True, "bytes": size})
        except FileNotFoundError:
            result.skipped += 1
        except PermissionError as exc:
            result.access_denied += 1
            result.details.append({"path": c.path, "success": False, "error": str(exc)})
        except OSError as exc:
            if getattr(exc, "winerror", None) in (32, 33):
                result.locked += 1
            else:
                result.errors += 1
            result.details.append({"path": c.path, "success": False, "error": str(exc)})

    if result.status != "cancelled":
        result.status = ("partial"
                         if result.errors or result.locked or result.access_denied
                         else "success")
    return result


# ============================================================================
#  КОРЗИНА И DNS
# ============================================================================
class SHQUERYRBINFO(ctypes.Structure):
    _fields_ = [
        ("cbSize", ctypes.c_ulong),
        ("i64Size", ctypes.c_longlong),
        ("i64NumItems", ctypes.c_longlong),
    ]


def drive_letters() -> List[str]:
    result = []
    if not IS_WINDOWS:
        return result
    try:
        mask = ctypes.windll.kernel32.GetLogicalDrives()
        for letter in string.ascii_uppercase:
            if mask & 1:
                try:
                    dtype = ctypes.windll.kernel32.GetDriveTypeW(f"{letter}:\\")
                except Exception:
                    dtype = 3
                if dtype in (2, 3):
                    result.append(f"{letter}:\\")
            mask >>= 1
    except Exception:
        log.exception("Ошибка получения дисков")
    return result


def recycle_size() -> Tuple[int, int]:
    """Возвращает (размер, количество)."""
    if not IS_WINDOWS:
        return 0, 0
    total, items = 0, 0
    for drive in drive_letters():
        try:
            info = SHQUERYRBINFO()
            info.cbSize = ctypes.sizeof(SHQUERYRBINFO)
            code = ctypes.windll.shell32.SHQueryRecycleBinW(
                ctypes.c_wchar_p(drive), ctypes.byref(info)
            )
            if code == 0:
                total += int(info.i64Size)
                items += int(info.i64NumItems)
        except Exception:
            continue
    return total, items


def empty_recycle_bin() -> OperationResult:
    result = OperationResult()
    size, items = recycle_size() if IS_WINDOWS else (0, 0)

    if IS_WINDOWS:
        try:
            flags = 0x1 | 0x2 | 0x4
            code = ctypes.windll.shell32.SHEmptyRecycleBinW(None, None, flags)
            if code == 0:
                result.deleted = items
                result.bytes_freed = size
                result.message = f"Корзина очищена ({items} элементов)"
            else:
                result.status = "failed"
                result.errors = 1
                result.message = f"Код WinAPI: {code}"
        except Exception as exc:
            result.status = "failed"
            result.errors = 1
            result.message = str(exc)
        return result

    if IS_LINUX:
        try:
            r = subprocess.run(["gio", "trash", "--empty"],
                               capture_output=True, timeout=30)
            if r.returncode == 0:
                result.message = "Корзина очищена"
                return result
        except Exception:
            pass
        try:
            trash = os.path.join(os.path.expanduser("~"), ".local", "share", "Trash")
            for sub in ("files", "info", "expunged"):
                d = os.path.join(trash, sub)
                if os.path.isdir(d):
                    for e in os.listdir(d):
                        p = os.path.join(d, e)
                        try:
                            if os.path.isdir(p) and not os.path.islink(p):
                                shutil.rmtree(p)
                            else:
                                os.remove(p)
                        except Exception:
                            pass
            result.message = "Корзина очищена"
            return result
        except Exception as exc:
            result.status = "failed"
            result.errors = 1
            result.message = str(exc)
        return result

    if IS_MAC:
        try:
            trash = os.path.join(os.path.expanduser("~"), ".Trash")
            if os.path.isdir(trash):
                for e in os.listdir(trash):
                    p = os.path.join(trash, e)
                    try:
                        if os.path.isdir(p) and not os.path.islink(p):
                            shutil.rmtree(p)
                        else:
                            os.remove(p)
                    except Exception:
                        pass
            result.message = "Корзина очищена"
        except Exception as exc:
            result.status = "failed"
            result.errors = 1
            result.message = str(exc)
        return result

    result.status = "failed"
    result.message = "Не поддерживается"
    return result


def flush_dns() -> OperationResult:
    result = OperationResult()
    if IS_WINDOWS:
        cmd = run_command(["ipconfig", "/flushdns"], 30)
    elif IS_LINUX:
        cmd = run_command(["systemd-resolve", "--flush-caches"], 30)
        if not cmd["success"]:
            cmd = run_command(["resolvectl", "flush-caches"], 30)
    elif IS_MAC:
        cmd = run_command(["dscacheutil", "-flushcache"], 30)
    else:
        cmd = {"success": False, "stderr": "Не поддерживается"}
    if cmd["success"]:
        result.deleted = 1
        result.message = "Кэш DNS очищен"
    else:
        result.status = "failed"
        result.errors = 1
        result.message = cmd.get("stderr") or cmd.get("stdout") or "Ошибка"
    return result

def _iter_files(roots: List[str], token: CancelToken,
                progress: Callable[[str], None]):
    """Генератор отдаёт os.DirEntry, чтобы избежать повторных stat()."""
    scanned = 0
    ignored = {str(x).lower() for x in SETTINGS["ignore_dirs"]}
    for root in normalize_paths(roots):
        if token.cancelled():
            return
        if not os.path.isdir(root) or is_reparse_point(root):
            continue
        stack = [root]
        while stack and not token.cancelled():
            current = stack.pop()
            try:
                with os.scandir(current) as it:
                    for entry in it:
                        if token.cancelled():
                            return
                        try:
                            if entry.name.lower() in ignored:
                                continue
                            if entry_is_reparse(entry):
                                continue
                            if entry.is_dir(follow_symlinks=False):
                                stack.append(entry.path)
                            elif entry.is_file(follow_symlinks=False):
                                scanned += 1
                                if scanned % 500 == 0:
                                    progress(f"Проверено файлов: {scanned}")
                                yield entry
                        except OSError:
                            continue
            except OSError:
                continue


def find_large_files(roots: List[str], min_mb: int, token: CancelToken,
                     progress: Callable[[str], None]) -> List[Dict[str, Any]]:
    threshold = int(min_mb) * 1024 * 1024
    result: List[Dict[str, Any]] = []
    count = 0
    for entry in _iter_files(roots, token, progress):
        if token.cancelled():
            break
        try:
            st = entry.stat(follow_symlinks=False)
            if st.st_size >= threshold:
                result.append({
                    "path": entry.path, "size": st.st_size,
                    "modified": st.st_mtime,
                })
                count += 1
                if count % 20 == 0:
                    progress(f"Найдено больших файлов: {count}")
        except OSError:
            continue
    result.sort(key=lambda x: x["size"], reverse=True)
    return result[:1000]


def partial_hash(path: str, size: int) -> Optional[str]:
    digest = hashlib.sha256()
    block = 1024 * 1024
    try:
        with open(path, "rb") as fh:
            if size <= block:
                # Файл маленький — одного чтения достаточно
                digest.update(fh.read())
            else:
                digest.update(fh.read(block))
                fh.seek(max(0, size - block))
                digest.update(fh.read(block))
        digest.update(str(size).encode("ascii"))
        return digest.hexdigest()
    except OSError:
        return None


def full_hash(path: str, token: CancelToken) -> Optional[str]:
    digest = hashlib.sha256()
    try:
        with open(path, "rb") as fh:
            while not token.cancelled():
                chunk = fh.read(2 * 1024 * 1024)
                if not chunk:
                    return digest.hexdigest()
                digest.update(chunk)
    except OSError:
        return None
    return None


def _file_id(path: str) -> Optional[Tuple[int, int]]:
    """(st_dev, st_ino) для учёта hard links."""
    try:
        st = os.stat(path, follow_symlinks=False)
        return (st.st_dev, st.st_ino)
    except OSError:
        return None


def find_duplicates(roots: List[str], min_mb: int, token: CancelToken,
                    progress: Callable[[str], None]) -> List[Dict[str, Any]]:
    threshold = int(min_mb) * 1024 * 1024
    sizes: Dict[int, List[str]] = {}

    for entry in _iter_files(roots, token, progress):
        if token.cancelled():
            return []
        try:
            st = entry.stat(follow_symlinks=False)
            if st.st_size >= threshold:
                sizes.setdefault(st.st_size, []).append(entry.path)
        except OSError:
            continue

    def _dedupe_by_inode(paths: List[str]) -> List[str]:
        seen: Set[Tuple[int, int]] = set()
        out: List[str] = []
        for p in paths:
            fid = _file_id(p)
            if fid is None or fid in seen:
                continue
            seen.add(fid)
            out.append(p)
        return out

    partial_groups: Dict[Tuple[int, str], List[str]] = {}
    checked = 0
    for size, paths in sizes.items():
        if token.cancelled():
            return []
        paths = _dedupe_by_inode(paths)
        if len(paths) < 2:
            continue
        for path in paths:
            if token.cancelled():
                return []
            checked += 1
            if checked % 20 == 0:
                progress(f"Быстрое сравнение: {checked}")
            digest = partial_hash(path, size)
            if digest:
                partial_groups.setdefault((size, digest), []).append(path)

    full_groups: Dict[Tuple[int, str], List[str]] = {}
    checked = 0
    for (size, _), paths in partial_groups.items():
        if token.cancelled():
            return []
        if len(paths) < 2:
            continue
        for path in paths:
            if token.cancelled():
                return []
            checked += 1
            if checked % 10 == 0:
                progress(f"Полное сравнение: {checked}")
            digest = full_hash(path, token)
            if digest:
                full_groups.setdefault((size, digest), []).append(path)

    result = []
    for (size, digest), paths in full_groups.items():
        if len(paths) >= 2:
            result.append({
                "size": size, "hash": digest, "files": paths,
                "wasted": size * (len(paths) - 1),
            })
    result.sort(key=lambda x: x["wasted"], reverse=True)
    return result[:500]


def _dir_size(path: str, stop_flag: Callable[[], bool] = lambda: False,
              error_counter: Optional[List[int]] = None
              ) -> Tuple[int, int]:
    total, count = 0, 0
    if error_counter is None:
        error_counter = [0]

    def _onerror(_e: OSError) -> None:
        error_counter[0] += 1

    for dp, dn, fn in os.walk(path, onerror=_onerror, followlinks=False):
        if stop_flag():
            break
        # Пропускаем reparse points в подкаталогах
        dn[:] = [d for d in dn
                 if not is_reparse_point(os.path.join(dp, d))]
        for f in fn:
            if stop_flag():
                break
            try:
                total += os.path.getsize(os.path.join(dp, f))
                count += 1
            except OSError:
                error_counter[0] += 1
    return total, count


def get_folder_sizes(root: str, stop_flag: Callable[[], bool] = lambda: False,
                     on_progress: Optional[Callable[[int, int, str], None]] = None
                     ) -> List[FolderSize]:
    result: List[FolderSize] = []
    if not os.path.isdir(root):
        return result
    try:
        entries = list(os.scandir(root))
    except OSError:
        return result

    root_files_size = 0
    root_files_count = 0
    dir_entries = []
    for e in entries:
        try:
            if entry_is_reparse(e):
                continue
            if e.is_dir(follow_symlinks=False):
                dir_entries.append(e)
            elif e.is_file(follow_symlinks=False):
                root_files_size += e.stat(follow_symlinks=False).st_size
                root_files_count += 1
        except OSError:
            continue

    total_dirs = len(dir_entries)
    for i, entry in enumerate(dir_entries):
        if stop_flag():
            break
        if on_progress:
            on_progress(i, total_dirs, entry.path)
        size, count = _dir_size(entry.path, stop_flag)
        result.append(FolderSize(path=entry.path, name=entry.name,
                                 size=size, files=count))

    result.sort(key=lambda x: x.size, reverse=True)
    if root_files_count > 0:
        result.insert(0, FolderSize(path=root, name="📄 (файлы в корне)",
                                     size=root_files_size, files=root_files_count,
                                     is_root_files=True))
    return result


# ============================================================================
#  СИСТЕМНАЯ ИНФОРМАЦИЯ
# ============================================================================
def system_snapshot() -> Dict[str, Any]:
    result: Dict[str, Any] = {
        "score": 50, "state": "Недоступно", "cpu": None, "memory": None,
        "disk_free": None, "disk_total": None, "warnings": [],
    }
    if psutil is None:
        result["warnings"].append("Установите psutil")
        return result
    try:
        cpu = psutil.cpu_percent(interval=0.15)
        memory = psutil.virtual_memory()
        system_drive = get_system_drive()
        disk = psutil.disk_usage(system_drive)
        score = 100
        warnings: List[str] = []
        free_percent = disk.free * 100 / disk.total if disk.total else 0
        if free_percent < 10:
            score -= 30
            warnings.append("На системном диске менее 10% свободного места")
        elif free_percent < 20:
            score -= 15
            warnings.append("На системном диске менее 20% свободного места")
        if memory.percent > 90:
            score -= 20
            warnings.append("Очень высокая загрузка памяти")
        elif memory.percent > 75:
            score -= 10
            warnings.append("Высокая загрузка памяти")
        if cpu > 90:
            score -= 15
            warnings.append("Очень высокая загрузка CPU")
        state = ("Отлично" if score >= 85 else
                 "Нормально" if score >= 65 else
                 "Есть замечания" if score >= 40 else
                 "Требует внимания")
        result.update({
            "score": max(0, score), "state": state, "cpu": cpu,
            "memory": memory.percent, "disk_free": disk.free,
            "disk_total": disk.total, "warnings": warnings,
        })
    except Exception as exc:
        result["warnings"].append(str(exc))
    return result


def list_processes() -> List[Dict[str, Any]]:
    if psutil is None:
        return []
    result = []
    for process in psutil.process_iter(["pid", "name", "username",
                                        "memory_info", "status"]):
        try:
            info = process.info
            memory = info.get("memory_info")
            result.append({
                "pid": info.get("pid", 0),
                "name": info.get("name") or "",
                "user": info.get("username") or "",
                "memory": memory.rss if memory else 0,
                "status": info.get("status") or "",
            })
        except Exception:
            continue
    result.sort(key=lambda x: x["memory"], reverse=True)
    return result


def terminate_process(pid: int) -> Tuple[bool, str]:
    if psutil is None:
        return False, "psutil не установлен"
    try:
        pid = int(pid)
        if pid <= 4 or pid == os.getpid():
            return False, "Этот процесс завершать запрещено"
        process = psutil.Process(pid)
        name = (process.name() or "").lower()
        if name in CRITICAL_PROCESS_NAMES:
            return False, f"Критический процесс: {name}"
        process.terminate()
        try:
            process.wait(4)
        except psutil.TimeoutExpired:
            process.kill()
        return True, "Процесс завершён"
    except psutil.NoSuchProcess:
        return True, "Процесс уже завершён"
    except psutil.AccessDenied:
        return False, "Недостаточно прав"
    except Exception as exc:
        return False, str(exc)


# --- Автозагрузка (Windows) --------------------------------------------------
def startup_folders() -> List[str]:
    program_data = os.environ.get("ProgramData", r"C:\ProgramData")
    return normalize_paths([
        os.path.join(get_user_profile(), "AppData", "Roaming", "Microsoft",
                     "Windows", "Start Menu", "Programs", "Startup"),
        os.path.join(program_data, "Microsoft", "Windows", "Start Menu",
                     "Programs", "Startup"),
    ])


def startup_views() -> List[Tuple[str, int]]:
    if not HAS_WINREG:
        return []
    result = [("HKCU", 0)]
    view64 = getattr(winreg, "KEY_WOW64_64KEY", 0)
    view32 = getattr(winreg, "KEY_WOW64_32KEY", 0)
    if sys.maxsize > 2**32:
        if view64:
            result.append(("HKLM", view64))
        if view32:
            result.append(("HKLM", view32))
    else:
        result.append(("HKLM", 0))
    return result


def registry_root(name: str):
    if not HAS_WINREG:
        return None
    return winreg.HKEY_CURRENT_USER if name == "HKCU" else winreg.HKEY_LOCAL_MACHINE


def list_startup_items() -> List[Dict[str, Any]]:
    result = []
    if HAS_WINREG:
        for root_name, view in startup_views():
            try:
                with winreg.OpenKey(registry_root(root_name), STARTUP_RUN_PATH, 0,
                                    winreg.KEY_READ | view) as key:
                    idx = 0
                    while True:
                        try:
                            name, value, reg_type = winreg.EnumValue(key, idx)
                            idx += 1
                            result.append({
                                "kind": "registry", "name": name, "value": value,
                                "reg_type": reg_type, "root": root_name,
                                "view": view,
                                "source": f"{root_name}\\{STARTUP_RUN_PATH}",
                            })
                        except OSError:
                            break
            except OSError:
                continue

    for folder in startup_folders():
        if not os.path.isdir(folder):
            continue
        try:
            for entry in os.scandir(folder):
                if entry.is_file(follow_symlinks=False):
                    result.append({
                        "kind": "file", "name": entry.name,
                        "value": entry.path, "path": entry.path,
                        "source": folder,
                    })
        except OSError:
            continue
    return result


def _encode_registry_value(value: Any, reg_type: int) -> Dict[str, Any]:
    if isinstance(value, bytes):
        return {"encoding": "base64",
                "value": base64.b64encode(value).decode("ascii"),
                "type": reg_type}
    return {"encoding": "json", "value": value, "type": reg_type}


def _decode_registry_value(data: Dict[str, Any]) -> Tuple[Any, int]:
    value = data.get("value")
    if data.get("encoding") == "base64":
        value = base64.b64decode(value)
    return value, int(data.get("type", 1))


def disable_startup(item: Dict[str, Any]) -> Tuple[bool, str]:
    """Атомарное отключение: сначала бэкап, потом действие."""
    backups = load_json(STARTUP_BACKUP_FILE, [])
    if not isinstance(backups, list):
        backups = []
    backup_id = datetime.now().strftime("%Y%m%d_%H%M%S_%f")

    if item.get("kind") == "registry":
        if not HAS_WINREG:
            return False, "winreg недоступен"
        root_name = item["root"]
        view = int(item.get("view", 0))
        backup = {
            "id": backup_id, "kind": "registry", "name": item["name"],
            "root": root_name, "view": view, "source": item["source"],
            "created": datetime.now().isoformat(timespec="seconds"),
            "data": _encode_registry_value(item["value"], item["reg_type"]),
        }
        # Сначала записываем бэкап
        backups.append(backup)
        if not save_json(STARTUP_BACKUP_FILE, backups):
            backups.pop()
            return False, "Не удалось сохранить бэкап"
        # Затем удаляем запись
        try:
            with winreg.OpenKey(registry_root(root_name), STARTUP_RUN_PATH, 0,
                                winreg.KEY_SET_VALUE | view) as key:
                winreg.DeleteValue(key, item["name"])
            return True, "Запись отключена"
        except Exception as exc:
            log.exception("Ошибка отключения автозагрузки")
            # Откатываем бэкап
            backups = [b for b in backups if b.get("id") != backup_id]
            save_json(STARTUP_BACKUP_FILE, backups)
            return False, str(exc)

    if item.get("kind") == "file":
        source = normalize_path(item.get("path", ""))
        if not source or not os.path.isfile(source):
            return False, "Файл не найден"
        target = os.path.join(
            str(STARTUP_BACKUP_DIR),
            f"{backup_id}_{safe_filename(os.path.basename(source))}"
        )
        # Сначала пробуем перенести — если упадёт, бэкап не нужен
        try:
            shutil.move(source, target)
        except Exception as exc:
            return False, str(exc)
        # Записываем бэкап; если не сохранится, откатываем файл обратно
        backups.append({
            "id": backup_id, "kind": "file", "name": item["name"],
            "original_path": source, "backup_path": target,
            "created": datetime.now().isoformat(timespec="seconds"),
        })
        if not save_json(STARTUP_BACKUP_FILE, backups):
            try:
                shutil.move(target, source)
            except Exception:
                log.exception("Не удалось откатить перенос файла")
            backups.pop()
            return False, "Не удалось сохранить бэкап"
        return True, "Файл отключён"
    return False, "Неизвестный тип"


def restore_startup(backup_id: str) -> Tuple[bool, str]:
    backups = load_json(STARTUP_BACKUP_FILE, [])
    backup = next((b for b in backups if b.get("id") == backup_id), None)
    if not backup:
        return False, "Резервная копия не найдена"
    try:
        if backup["kind"] == "registry":
            if not HAS_WINREG:
                return False, "winreg недоступен"
            value, reg_type = _decode_registry_value(backup["data"])
            with winreg.CreateKeyEx(registry_root(backup["root"]),
                                    STARTUP_RUN_PATH, 0,
                                    winreg.KEY_SET_VALUE
                                    | int(backup.get("view", 0))) as key:
                winreg.SetValueEx(key, backup["name"], 0, reg_type, value)
        elif backup["kind"] == "file":
            source = backup["backup_path"]
            target = backup["original_path"]
            if os.path.exists(target):
                return False, "Исходный файл уже существует"
            os.makedirs(os.path.dirname(target), exist_ok=True)
            shutil.move(source, target)
        backups = [b for b in backups if b.get("id") != backup_id]
        save_json(STARTUP_BACKUP_FILE, backups)
        return True, "Элемент восстановлен"
    except Exception as exc:
        return False, str(exc)


# --- JSON helpers ------------------------------------------------------------
def load_json(path, default):
    try:
        if not os.path.isfile(str(path)):
            return default
        with open(path, "r", encoding="utf-8") as fh:
            return json.load(fh)
    except Exception:
        log.exception("Не удалось прочитать JSON: %s", path)
        return default


def save_json(path, data) -> bool:
    path_str = str(path)
    directory = os.path.dirname(path_str)
    tmp = path_str + ".tmp"
    try:
        if directory:
            os.makedirs(directory, exist_ok=True)
        with open(tmp, "w", encoding="utf-8", newline="\n") as fh:
            json.dump(data, fh, ensure_ascii=False, indent=2)
        os.replace(tmp, path_str)
        return True
    except Exception:
        log.exception("Не удалось сохранить JSON: %s", path)
        try:
            if os.path.isfile(tmp):
                os.remove(tmp)
        except OSError:
            pass
        return False


def save_result_report(name: str, result: OperationResult) -> str:
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    path = REPORT_DIR / f"{safe_filename(name)}_{timestamp}.json"
    save_json(path, {
        "application": APP_NAME, "version": APP_VERSION,
        "time": datetime.now().isoformat(timespec="seconds"),
        "result": asdict(result),
    })
    return str(path)


# ============================================================================
#  МОДЕЛИ ТАБЛИЦЫ ФАЙЛОВ
# ============================================================================
_COL_NAME, _COL_PATH, _COL_CAT, _COL_SIZE, _COL_MTIME = 0, 1, 2, 3, 4
_HEADERS = ["Имя", "Путь", "Категория", "Размер", "Изменён"]


class FileTableModel(QAbstractTableModel):
    def __init__(self, files: Optional[List[FileInfo]] = None) -> None:
        super().__init__()
        self._files: List[FileInfo] = files or []

    def set_files(self, files: List[FileInfo]) -> None:
        self.beginResetModel()
        self._files = files
        self.endResetModel()

    def remove_files(self, normalized_paths: Set[str]) -> None:
        if not normalized_paths:
            return
        self.beginResetModel()
        self._files = [f for f in self._files
                       if f.normalized_path not in normalized_paths]
        self.endResetModel()

    def file_at(self, row: int) -> Optional[FileInfo]:
        return self._files[row] if 0 <= row < len(self._files) else None

    def rowCount(self, parent=QModelIndex()) -> int:
        return 0 if parent.isValid() else len(self._files)

    def columnCount(self, parent=QModelIndex()) -> int:
        return 0 if parent.isValid() else len(_HEADERS)

    def headerData(self, section, orientation, role=Qt.ItemDataRole.DisplayRole):
        if (role == Qt.ItemDataRole.DisplayRole
                and orientation == Qt.Orientation.Horizontal
                and 0 <= section < len(_HEADERS)):
            return _HEADERS[section]
        return None

    def flags(self, index):
        if not index.isValid():
            return Qt.ItemFlag.NoItemFlags
        return Qt.ItemFlag.ItemIsEnabled | Qt.ItemFlag.ItemIsSelectable

    def data(self, index, role=Qt.ItemDataRole.DisplayRole):
        if not index.isValid():
            return None
        f = self.file_at(index.row())
        if f is None:
            return None
        col = index.column()
        if role == Qt.ItemDataRole.DisplayRole:
            return {0: f.name, 1: f.path, 2: f.category,
                    3: human_size(f.size), 4: format_dt(f.mtime)}.get(col)
        if role == Qt.ItemDataRole.ToolTipRole:
            return f.path
        if role == Qt.ItemDataRole.TextAlignmentRole:
            align = Qt.AlignmentFlag.AlignVCenter
            if col in (_COL_SIZE, _COL_MTIME):
                return align | Qt.AlignmentFlag.AlignRight
            return align | Qt.AlignmentFlag.AlignLeft
        if role == Qt.ItemDataRole.UserRole:
            return {0: f.name.lower(), 1: f.path.lower(),
                    2: f.category.lower(), 3: f.size, 4: f.mtime}.get(col)
        return None


class FileFilterProxy(QSortFilterProxyModel):
    def __init__(self) -> None:
        super().__init__()
        self._text = ""
        self._folder_norm = ""
        self._category = ""
        self.setSortRole(Qt.ItemDataRole.UserRole)

    def set_text_filter(self, text: str) -> None:
        self._text = text.strip().lower()
        self.invalidateFilter()

    def set_folder_filter(self, folder: str) -> None:
        self._folder_norm = normalize_path(folder) if folder else ""
        self.invalidateFilter()

    def set_category_filter(self, category: str) -> None:
        self._category = category
        self.invalidateFilter()

    def filterAcceptsRow(self, source_row, source_parent):
        model: FileTableModel = self.sourceModel()  # type: ignore
        if model is None:
            return True
        f = model.file_at(source_row)
        if f is None:
            return False
        if self._folder_norm and not is_inside(f.normalized_path,
                                               self._folder_norm,
                                               allow_equal=True):
            return False
        if self._category and f.category != self._category:
            return False
        if self._text and self._text not in f.search_blob:
            return False
        return True


# ============================================================================
#  ФОНОВЫЕ ПОТОКИ (QThread)
# ============================================================================
class ScanWorker(QThread):
    progress = pyqtSignal(int, int, str)
    finished_ok = pyqtSignal(object)
    finished_err = pyqtSignal(str)
    canceled = pyqtSignal()

    def __init__(self, root_path: str) -> None:
        super().__init__()
        self.root_path = root_path

    def request_stop(self) -> None:
        self.requestInterruption()

    def run(self) -> None:
        try:
            result = scan_disk(self.root_path,
                               progress_callback=self.progress.emit,
                               stop_requested=self.isInterruptionRequested)
        except ScanCanceled:
            self.canceled.emit()
        except Exception as exc:
            log.exception("Ошибка сканирования")
            self.finished_err.emit(str(exc))
        else:
            self.finished_ok.emit(result)


class WorkerThread(QThread):
    progress = pyqtSignal(str)
    completed = pyqtSignal(str, object)
    failed = pyqtSignal(str, str)

    def __init__(self, operation: str, payload: Optional[Dict[str, Any]] = None,
                 parent=None) -> None:
        super().__init__(parent)
        self.operation = operation
        self.payload = payload or {}
        self.token = CancelToken()

    def cancel(self) -> None:
        self.token.cancel()
        self.requestInterruption()

    def report(self, message: str) -> None:
        self.progress.emit(str(message))

    def run(self) -> None:
        try:
            op = self.operation
            if op == "health":
                data = system_snapshot()
            elif op == "cleanup_scan":
                data = scan_cleanup(self.payload.get("categories", []),
                                    int(self.payload.get("age", 72)),
                                    self.token, self.report)
            elif op == "cleanup_delete":
                data = delete_candidates(self.payload.get("candidates", []),
                                         self.token, self.report)
            elif op == "big_files":
                data = find_large_files(self.payload.get("roots", []),
                                        int(self.payload.get("min_mb", 250)),
                                        self.token, self.report)
            elif op == "duplicates":
                data = find_duplicates(self.payload.get("roots", []),
                                       int(self.payload.get("min_mb", 20)),
                                       self.token, self.report)
            elif op == "folder_sizes":
                data = get_folder_sizes(
                    self.payload.get("root", ""),
                    lambda: self.token.cancelled(),
                    lambda i, t, p: self.report(
                        f"Анализ {i+1}/{t}: {os.path.basename(p)}"))
            elif op == "processes":
                data = list_processes()
            elif op == "startup":
                data = list_startup_items()
            elif op == "recycle":
                data = empty_recycle_bin()
            elif op == "dns":
                data = flush_dns()
            else:
                raise RuntimeError(f"Неизвестная операция: {op}")
            self.completed.emit(op, data)
        except Exception as exc:
            log.exception("Ошибка фоновой операции %s", self.operation)
            self.failed.emit(self.operation, str(exc))


# ============================================================================
#  ГЛАВНОЕ ОКНО
# ============================================================================
class MainWindow(QMainWindow):

    ALL_CATEGORIES = "Все категории"

    def __init__(self) -> None:
        super().__init__()
        self.setWindowTitle(f"{APP_NAME} v{APP_VERSION}")
        self.resize(1350, 840)

        self._qsettings = QSettings(ORG_NAME, "PC_Optimizer")
        self._scan_worker: Optional[ScanWorker] = None
        self._worker: Optional[WorkerThread] = None

        # Сканер диска
        self._root_display = ""
        self._current_folder = ""
        self._folder_stats: Dict[str, FolderStats] = {}
        self._category_stats: Dict[str, CategoryStats] = {}
        self._tree_nodes: Dict[str, QTreeWidgetItem] = {}
        self._total_bytes = 0
        self._scan_start_time: float = 0.0

        # Очистка
        self._preview_candidates: List[Candidate] = []

        # Анализ папок
        self._fs_root = ""
        self._fs_items: List[FolderSize] = []

        # Дубликаты / большие файлы
        self._dup_groups: List[Dict[str, Any]] = []
        self._large_items: List[Dict[str, Any]] = []

        # Модель файлов
        self._model = FileTableModel()
        self._proxy = FileFilterProxy()
        self._proxy.setSourceModel(self._model)

        self._build_menu()
        self._build_ui()
        self._apply_style()
        self._restore_geometry()

        QTimer.singleShot(300, self._refresh_dashboard)

    # ---------------------------------------------------------------- меню
    def _build_menu(self) -> None:
        bar = self.menuBar()
        file_menu = bar.addMenu("&Файл")

        act_rescan = QAction("Пересканировать", self)
        act_rescan.setShortcut(QKeySequence("F5"))
        act_rescan.triggered.connect(self._start_disk_scan)
        file_menu.addAction(act_rescan)
        self.addAction(act_rescan)

        act_export = QAction("Экспорт текущего вида в CSV…", self)
        act_export.triggered.connect(self._export_csv)
        file_menu.addAction(act_export)

        file_menu.addSeparator()
        act_exit = QAction("Выход", self)
        act_exit.setShortcut(QKeySequence("Ctrl+Q"))
        act_exit.triggered.connect(self.close)
        file_menu.addAction(act_exit)

        help_menu = bar.addMenu("&Справка")
        act_about = QAction("О программе", self)
        act_about.triggered.connect(self._show_about)
        help_menu.addAction(act_about)

        act_focus_search = QAction(self)
        act_focus_search.setShortcut(QKeySequence.StandardKey.Find)
        act_focus_search.triggered.connect(self._focus_search)
        self.addAction(act_focus_search)

        act_cancel = QAction(self)
        act_cancel.setShortcut(QKeySequence("Esc"))
        act_cancel.triggered.connect(self._cancel_everything)
        self.addAction(act_cancel)

    def _show_about(self) -> None:
        trash_note = ("включена (send2trash)" if HAS_SEND2TRASH
                      else "недоступна — удаление безвозвратное")
        QMessageBox.about(
            self, "О программе",
            f"{APP_NAME} v{APP_VERSION}\n\n"
            "Объединённая утилита: сканер диска, анализ размеров папок,\n"
            "очистка системного мусора, поиск дубликатов и больших файлов,\n"
            "управление автозагрузкой и процессами.\n\n"
            f"Корзина: {trash_note}\n\n"
            "Горячие клавиши:\n"
            "F5 — пересканировать диск\n"
            "Ctrl+F — поиск\n"
            "Esc — отменить текущую операцию\n"
            "Delete — удалить выбранные файлы (в таблице файлов)"
        )

    def _focus_search(self) -> None:
        if self.tabs.currentWidget() is self.tab_disk:
            self._search_edit.setFocus()
            self._search_edit.selectAll()

    # ---------------------------------------------------------------- UI
    def _build_ui(self) -> None:
        root = QWidget()
        self.setCentralWidget(root)
        layout = QVBoxLayout(root)
        layout.setContentsMargins(8, 8, 8, 8)

        self.tabs = QTabWidget()
        layout.addWidget(self.tabs)

        self._build_dashboard_tab()
        self._build_disk_tab()
        self._build_cleanup_tab()
        self._build_folder_size_tab()
        self._build_duplicates_tab()
        self._build_large_tab()
        self._build_processes_tab()
        self._build_startup_tab()
        self._build_settings_tab()

        self.tabs.currentChanged.connect(self._on_tab_changed)

        self.status_bar = QStatusBar()
        self.setStatusBar(self.status_bar)
        self.status_bar.showMessage("Готово")

        status_widget = QWidget()
        status_layout = QHBoxLayout(status_widget)
        status_layout.setContentsMargins(0, 0, 0, 0)
        self._op_label = QLabel("Нет активных операций")
        self._op_progress = QProgressBar()
        self._op_progress.setRange(0, 1)
        self._op_progress.setValue(0)
        self._op_progress.setFixedWidth(220)
        self._cancel_btn = QPushButton("Отмена")
        self._cancel_btn.setEnabled(False)
        self._cancel_btn.clicked.connect(self._cancel_everything)
        status_layout.addWidget(self._op_label, 1)
        status_layout.addWidget(self._op_progress)
        status_layout.addWidget(self._cancel_btn)
        self.status_bar.addPermanentWidget(status_widget)

    def _apply_style(self) -> None:
        QApplication.setStyle("Fusion")
        self.setStyleSheet("""
            QMainWindow  { background: #f5f7fb; }
            QLabel       { color: #1f2937; }
            QLineEdit, QComboBox, QSpinBox {
                padding: 6px 9px; border: 1px solid #cfd8e3;
                border-radius: 6px; background: white;
            }
            QPushButton {
                padding: 6px 12px; border-radius: 6px;
                background: #e9eef5; border: 1px solid #cfd8e3;
            }
            QPushButton:hover    { background: #dde6f2; }
            QPushButton:disabled { color: #94a3b8; background: #f1f5f9; }
            QTreeWidget, QTableView, QTableWidget {
                background: white; border: 1px solid #d9e1eb;
                alternate-background-color: #f8fafc;
            }
            QHeaderView::section {
                background: #eaf0f8; padding: 5px; border: none;
                border-bottom: 1px solid #d9e1eb;
                border-right: 1px solid #d9e1eb; font-weight: 600;
            }
            QProgressBar {
                border: 1px solid #d9e1eb; border-radius: 5px;
                text-align: center; background: white; height: 14px;
            }
            QProgressBar::chunk { background-color: #4f8cff; border-radius: 5px; }
            QTabWidget::pane { border: 1px solid #d9e1eb; background: #f5f7fb; }
            QTabBar::tab { background: #e9eef5; padding: 7px 14px;
                           border: 1px solid #cfd8e3; margin-right: 2px; }
            QTabBar::tab:selected { background: #4f8cff; color: white; }
        """)

    # ------------------------------------------------------------- Geometry
    def _restore_geometry(self) -> None:
        geo = SETTINGS["geometry"]
        state = SETTINGS["window_state"]
        try:
            if geo:
                from PyQt6.QtCore import QByteArray
                self.restoreGeometry(QByteArray(geo))
            if state:
                from PyQt6.QtCore import QByteArray
                self.restoreState(QByteArray(state))
        except Exception:
            log.exception("Не удалось восстановить геометрию окна")

    def _save_geometry(self) -> None:
        try:
            SETTINGS["geometry"] = bytes(self.saveGeometry()).decode("latin-1")
            SETTINGS["window_state"] = bytes(self.saveState()).decode("latin-1")
        except Exception:
            log.exception("Не удалось сохранить геометрию окна")

    # ------------------------------------------------------------- Dashboard
    def _build_dashboard_tab(self) -> None:
        tab = QWidget()
        layout = QVBoxLayout(tab)
        layout.setContentsMargins(16, 16, 16, 16)

        title = QLabel("Дашборд")
        title.setStyleSheet("font-size: 22px; font-weight: 800;")
        layout.addWidget(title)

        grid = QGridLayout()
        self._dash_health, self._dash_health_val = self._make_metric("Здоровье системы")
        self._dash_disk,   self._dash_disk_val   = self._make_metric("Свободно на диске")
        self._dash_mem,    self._dash_mem_val    = self._make_metric("Оперативная память")
        self._dash_cpu,    self._dash_cpu_val    = self._make_metric("CPU")
        self._dash_clean,  self._dash_clean_val  = self._make_metric("Последняя очистка")
        self._dash_freed,  self._dash_freed_val  = self._make_metric("Освобождено (последняя)")

        grid.addWidget(self._dash_health, 0, 0)
        grid.addWidget(self._dash_disk,   0, 1)
        grid.addWidget(self._dash_mem,    0, 2)
        grid.addWidget(self._dash_cpu,    1, 0)
        grid.addWidget(self._dash_clean,  1, 1)
        grid.addWidget(self._dash_freed,  1, 2)
        layout.addLayout(grid)

        self._health_bar = QProgressBar()
        self._health_bar.setRange(0, 100)
        self._health_bar.setValue(0)
        self._health_bar.setFormat("Здоровье: %p%")
        layout.addWidget(self._health_bar)

        self._warning_tree = QTreeWidget()
        self._warning_tree.setHeaderLabels(["Предупреждения"])
        self._warning_tree.header().setSectionResizeMode(
            0, QHeaderView.ResizeMode.Stretch)
        layout.addWidget(self._warning_tree)

        row = QHBoxLayout()
        refresh = QPushButton("🔄 Обновить")
        refresh.clicked.connect(self._refresh_dashboard)
        to_clean = QPushButton("🧹 Перейти к очистке")
        to_clean.clicked.connect(lambda: self.tabs.setCurrentWidget(self.tab_cleanup))
        to_disk = QPushButton("💾 Сканер диска")
        to_disk.clicked.connect(lambda: self.tabs.setCurrentWidget(self.tab_disk))
        row.addWidget(refresh)
        row.addWidget(to_clean)
        row.addWidget(to_disk)
        row.addStretch()
        layout.addLayout(row)

        self.tab_dashboard = tab
        self.tabs.addTab(tab, "Дашборд")

    def _make_metric(self, name: str):
        box = QGroupBox(name)
        v = QVBoxLayout(box)
        label = QLabel("—")
        label.setStyleSheet("font-size: 20px; font-weight: 800;")
        v.addWidget(label)
        return box, label

    def _refresh_dashboard(self) -> None:
        if self._is_busy():
            return
        self._start_worker("health")

    def _show_health(self, data: Dict[str, Any]) -> None:
        score = int(data.get("score", 0))
        self._dash_health_val.setText(f"{score}/100 — {data.get('state', '—')}")
        self._health_bar.setValue(score)
        disk_free = data.get("disk_free")
        disk_total = data.get("disk_total")
        self._dash_disk_val.setText(
            human_size(disk_free) if disk_free is not None else "N/A")
        if disk_total is not None:
            self._dash_disk.setToolTip(f"Всего: {human_size(disk_total)}")
        mem = data.get("memory")
        self._dash_mem_val.setText(f"{mem:.0f}%" if mem is not None else "N/A")
        cpu = data.get("cpu")
        self._dash_cpu_val.setText(f"{cpu:.0f}%" if cpu is not None else "N/A")
        self._dash_clean_val.setText(SETTINGS["last_cleanup"] or "Никогда")
        self._dash_freed_val.setText(human_size(SETTINGS["last_freed"]))

        self._warning_tree.clear()
        warnings = data.get("warnings") or []
        if warnings:
            for w in warnings:
                self._warning_tree.addTopLevelItem(QTreeWidgetItem([f"⚠ {w}"]))
        else:
            self._warning_tree.addTopLevelItem(
                QTreeWidgetItem(["Серьёзных предупреждений не обнаружено"]))

    # -------------------------------------------------------------- Сканер диска
    def _build_disk_tab(self) -> None:
        tab = QWidget()
        outer = QVBoxLayout(tab)
        outer.setContentsMargins(8, 8, 8, 8)

        top = QVBoxLayout()
        path_row = QHBoxLayout()
        self._path_edit = QLineEdit(
            SETTINGS["last_scan_path"] or os.path.expanduser("~"))
        self._path_edit.setPlaceholderText("Путь к диску или папке…")
        self._path_edit.setClearButtonEnabled(True)
        browse = QPushButton("📂 Обзор…")
        browse.clicked.connect(self._choose_path)
        scan_btn = QPushButton("🔍 Сканировать")
        scan_btn.clicked.connect(self._start_disk_scan)
        path_row.addWidget(QLabel("Путь:"))
        path_row.addWidget(self._path_edit, 1)
        path_row.addWidget(browse)
        path_row.addWidget(scan_btn)

        search_row = QHBoxLayout()
        self._search_edit = QLineEdit()
        self._search_edit.setPlaceholderText("Поиск по имени, пути или категории…")
        self._search_edit.setClearButtonEnabled(True)
        self._search_edit.textChanged.connect(self._on_search_changed)
        self._category_combo = QComboBox()
        self._category_combo.addItem(self.ALL_CATEGORIES)
        self._category_combo.addItems(ALL_CATEGORY_NAMES)
        self._category_combo.currentTextChanged.connect(self._on_category_changed)
        search_row.addWidget(QLabel("Поиск:"))
        search_row.addWidget(self._search_edit, 1)
        search_row.addWidget(QLabel("Категория:"))
        search_row.addWidget(self._category_combo)

        self._disk_summary = QLabel("Выберите папку и нажмите «Сканировать».")
        self._disk_summary.setStyleSheet("font-weight: 600;")
        self._disk_summary.setWordWrap(True)

        self._disk_progress = QProgressBar()
        self._disk_progress.setRange(0, 1)
        self._disk_progress.setValue(0)
        self._disk_progress.setFormat("Готово")

        top.addLayout(path_row)
        top.addLayout(search_row)
        top.addWidget(self._disk_summary)
        top.addWidget(self._disk_progress)
        outer.addLayout(top)

        splitter = QSplitter(Qt.Orientation.Horizontal)

        left = QWidget()
        left_layout = QVBoxLayout(left)
        left_layout.setContentsMargins(0, 0, 0, 0)
        left_splitter = QSplitter(Qt.Orientation.Vertical)

        tree_wrap = QWidget()
        tree_layout = QVBoxLayout(tree_wrap)
        tree_layout.setContentsMargins(0, 0, 0, 0)
        tree_layout.addWidget(QLabel("Дерево папок"))
        self._tree = QTreeWidget()
        self._tree.setHeaderLabels(["Папка", "Файлов", "Размер"])
        self._tree.setAlternatingRowColors(True)
        self._tree.setUniformRowHeights(True)
        self._tree.setSelectionMode(QAbstractItemView.SelectionMode.SingleSelection)
        self._tree.itemSelectionChanged.connect(self._on_tree_selection)
        self._tree.header().setSectionResizeMode(0, QHeaderView.ResizeMode.Stretch)
        self._tree.header().setSectionResizeMode(
            1, QHeaderView.ResizeMode.ResizeToContents)
        self._tree.header().setSectionResizeMode(
            2, QHeaderView.ResizeMode.ResizeToContents)
        tree_layout.addWidget(self._tree)

        cat_wrap = QWidget()
        cat_layout = QVBoxLayout(cat_wrap)
        cat_layout.setContentsMargins(0, 0, 0, 0)
        cat_layout.addWidget(QLabel("Категории"))
        self._category_table = QTableWidget(0, 4)
        self._category_table.setHorizontalHeaderLabels(
            ["Категория", "Файлов", "Размер", "%"])
        self._category_table.verticalHeader().setVisible(False)
        self._category_table.setEditTriggers(
            QAbstractItemView.EditTrigger.NoEditTriggers)
        self._category_table.setSelectionMode(
            QAbstractItemView.SelectionMode.NoSelection)
        self._category_table.setAlternatingRowColors(True)
        self._category_table.horizontalHeader().setSectionResizeMode(
            0, QHeaderView.ResizeMode.Stretch)
        for i in (1, 2, 3):
            self._category_table.horizontalHeader().setSectionResizeMode(
                i, QHeaderView.ResizeMode.ResizeToContents)
        cat_layout.addWidget(self._category_table)

        left_splitter.addWidget(tree_wrap)
        left_splitter.addWidget(cat_wrap)
        left_splitter.setStretchFactor(0, 3)
        left_splitter.setStretchFactor(1, 2)
        left_layout.addWidget(left_splitter)

        right = QWidget()
        right_layout = QVBoxLayout(right)
        right_layout.setContentsMargins(0, 0, 0, 0)
        right_layout.addWidget(QLabel("Файлы"))
        self._table = QTableView()
        self._table.setModel(self._proxy)
        self._table.setAlternatingRowColors(True)
        self._table.setSelectionBehavior(
            QAbstractItemView.SelectionBehavior.SelectRows)
        self._table.setSelectionMode(
            QAbstractItemView.SelectionMode.ExtendedSelection)
        self._table.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self._table.setSortingEnabled(True)
        self._table.verticalHeader().setVisible(False)
        self._table.activated.connect(self._on_table_activated)
        self._table.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self._table.customContextMenuRequested.connect(self._show_table_context_menu)
        act_delete = QAction("Удалить", self._table)
        act_delete.setShortcut(QKeySequence.StandardKey.Delete)
        act_delete.setShortcutContext(Qt.ShortcutContext.WidgetShortcut)
        act_delete.triggered.connect(self._delete_selected_files)
        self._table.addAction(act_delete)
        hh = self._table.horizontalHeader()
        hh.setSectionResizeMode(_COL_NAME, QHeaderView.ResizeMode.ResizeToContents)
        hh.setSectionResizeMode(_COL_PATH, QHeaderView.ResizeMode.Stretch)
        hh.setSectionResizeMode(_COL_CAT, QHeaderView.ResizeMode.ResizeToContents)
        hh.setSectionResizeMode(_COL_SIZE, QHeaderView.ResizeMode.ResizeToContents)
        hh.setSectionResizeMode(_COL_MTIME, QHeaderView.ResizeMode.ResizeToContents)
        self._table.sortByColumn(_COL_SIZE, Qt.SortOrder.DescendingOrder)
        right_layout.addWidget(self._table)

        splitter.addWidget(left)
        splitter.addWidget(right)
        splitter.setStretchFactor(0, 1)
        splitter.setStretchFactor(1, 3)
        splitter.setSizes([340, 950])

        outer.addWidget(splitter, 1)
        self.tab_disk = tab
        self.tabs.addTab(tab, "Сканер диска")

    def _choose_path(self) -> None:
        start = self._path_edit.text().strip() or os.path.expanduser("~")
        path = QFileDialog.getExistingDirectory(self, "Выберите диск или папку", start)
        if path:
            self._path_edit.setText(path)

    def _start_disk_scan(self) -> None:
        if self._scan_worker and self._scan_worker.isRunning():
            return
        if self._is_busy():
            return
        root = self._path_edit.text().strip()
        if not root:
            QMessageBox.warning(self, "Нет пути", "Сначала выберите папку.")
            return
        if not os.path.exists(root):
            QMessageBox.critical(self, "Ошибка", f"Путь не найден:\n{root}")
            return
        if not os.path.isdir(root):
            QMessageBox.critical(self, "Ошибка", f"Путь не является папкой:\n{root}")
            return

        self._root_display = ""
        self._folder_stats = {}
        self._category_stats = {}
        self._tree_nodes = {}
        self._total_bytes = 0
        self._model.set_files([])
        self._tree.clear()
        self._category_table.setRowCount(0)
        self._proxy.set_folder_filter("")
        self._proxy.set_category_filter("")
        self._proxy.set_text_filter(self._search_edit.text())

        self._disk_progress.setRange(0, 0)
        self._disk_progress.setFormat("Сканирование…")
        self._disk_summary.setText("Сканирование…")
        self._scan_start_time = time.monotonic()
        self._cancel_btn.setEnabled(True)
        self._op_label.setText("Сканирование диска…")
        self._op_progress.setRange(0, 0)

        self._scan_worker = ScanWorker(root)
        self._scan_worker.progress.connect(self._on_disk_progress)
        self._scan_worker.finished_ok.connect(self._on_disk_finished)
        self._scan_worker.finished_err.connect(self._on_disk_error)
        self._scan_worker.canceled.connect(self._on_disk_canceled)
        self._scan_worker.start()
        self.status_bar.showMessage("Сканирование диска…")

    def _on_disk_progress(self, files: int, size: int, path: str) -> None:
        elapsed = (time.monotonic() - self._scan_start_time
                   if self._scan_start_time else 0)
        rate = files / elapsed if elapsed > 0.5 else 0
        rate_part = f" | {rate:.0f} ф/с" if rate > 0 else ""
        self._disk_progress.setFormat(
            f"Файлов: {files} | {human_size(size)}{rate_part}")
        self.status_bar.showMessage(f"Сканирование: {path}")

    def _on_disk_finished(self, result: ScanResult) -> None:
        self._root_display = result.root_display
        self._current_folder = result.root_display
        self._folder_stats = result.folder_stats
        self._category_stats = result.category_stats
        self._total_bytes = result.total_bytes
        self._model.set_files(result.files)
        self._build_tree(result.folder_stats, result.root_display)
        self._refresh_category_table()
        self._proxy.set_folder_filter(result.root_display)

        self._disk_progress.setRange(0, 1)
        self._disk_progress.setValue(1)
        self._disk_progress.setFormat("Готово")
        self._cancel_btn.setEnabled(False)
        self._op_progress.setRange(0, 1)
        self._op_progress.setValue(1)
        self._op_label.setText("Готово")

        elapsed = time.monotonic() - self._scan_start_time
        err_part = (f" | Ошибок доступа: {result.scan_errors}"
                    if result.scan_errors else "")
        self._disk_summary.setText(
            f"Файлов: {result.total_files} | Папок: {len(result.folder_stats)} | "
            f"Размер: {human_size(result.total_bytes)} | "
            f"Время: {elapsed:.1f} с{err_part}"
        )
        self._update_statusbar()
        self._cleanup_scan_worker()

    def _on_disk_error(self, message: str) -> None:
        self._disk_progress.setFormat("Ошибка")
        self._disk_summary.setText("Ошибка сканирования.")
        self._cancel_btn.setEnabled(False)
        self._op_progress.setRange(0, 1)
        self._op_progress.setValue(0)
        self._cleanup_scan_worker()
        QMessageBox.critical(self, "Ошибка сканирования", message)

    def _on_disk_canceled(self) -> None:
        self._disk_progress.setFormat("Отменено")
        self._disk_summary.setText("Сканирование отменено.")
        self._cancel_btn.setEnabled(False)
        self._op_progress.setRange(0, 1)
        self._op_progress.setValue(0)
        self._op_label.setText("Отменено")
        self._cleanup_scan_worker()

    def _cleanup_scan_worker(self) -> None:
        if self._scan_worker is not None:
            self._scan_worker.deleteLater()
            self._scan_worker = None

    def _build_tree(self, folder_stats: Dict[str, FolderStats],
                    root_display: str) -> None:
        self._tree.blockSignals(True)
        self._tree.clear()
        root_key = normalize_path(root_display)
        nodes: Dict[str, QTreeWidgetItem] = {}
        root_stats = (folder_stats.get(root_key)
                      or FolderStats(display=root_display, parent_key=None))
        root_item = QTreeWidgetItem([
            root_stats.display, str(root_stats.count), human_size(root_stats.size_bytes)
        ])
        root_item.setData(0, Qt.ItemDataRole.UserRole, root_stats.display)
        bold = QFont()
        bold.setBold(True)
        for col in range(3):
            root_item.setFont(col, bold)
        self._tree.addTopLevelItem(root_item)
        nodes[root_key] = root_item

        for key, stats in sorted(folder_stats.items(),
                                 key=lambda kv: path_depth(kv[1].display,
                                                           root_display)):
            if key == root_key or key in nodes:
                continue
            parent_key = stats.parent_key or root_key
            parent_item = nodes.get(parent_key, root_item)
            item = QTreeWidgetItem([
                folder_label(stats.display, root_display),
                str(stats.count), human_size(stats.size_bytes),
            ])
            item.setData(0, Qt.ItemDataRole.UserRole, stats.display)
            parent_item.addChild(item)
            nodes[key] = item

        root_item.setExpanded(True)
        self._tree.expandToDepth(1)
        self._tree.blockSignals(False)
        self._tree.setCurrentItem(root_item)
        self._tree_nodes = nodes

    def _refresh_category_table(self) -> None:
        self._category_table.setRowCount(0)
        total = max(self._total_bytes, 1)
        rows = sorted(self._category_stats.items(),
                      key=lambda kv: kv[1].size_bytes, reverse=True)
        self._category_table.setRowCount(len(rows))
        for row, (cat, stats) in enumerate(rows):
            pct = stats.size_bytes / total * 100
            self._category_table.setItem(row, 0, QTableWidgetItem(cat))
            self._category_table.setItem(row, 1, QTableWidgetItem(str(stats.count)))
            self._category_table.setItem(row, 2, QTableWidgetItem(
                human_size(stats.size_bytes)))
            self._category_table.setItem(row, 3, QTableWidgetItem(f"{pct:.1f}%"))

    def _on_tree_selection(self) -> None:
        if not self._root_display:
            return
        items = self._tree.selectedItems()
        folder = (items[0].data(0, Qt.ItemDataRole.UserRole)
                  if items else self._root_display)
        self._current_folder = folder or self._root_display
        self._proxy.set_folder_filter(self._current_folder)
        self._update_statusbar()

    def _on_search_changed(self, text: str) -> None:
        self._proxy.set_text_filter(text)
        self._update_statusbar()

    def _on_category_changed(self, text: str) -> None:
        self._proxy.set_category_filter("" if text == self.ALL_CATEGORIES else text)
        self._update_statusbar()

    def _update_statusbar(self) -> None:
        if not self._root_display:
            return
        visible = self._proxy.rowCount()
        total = self._model.rowCount()
        self.status_bar.showMessage(
            f"Папка: {self._current_folder or self._root_display} | "
            f"Показано: {visible} из {total} файлов"
        )

    # --- контекстное меню таблицы файлов
    def _selected_files(self) -> List[FileInfo]:
        result, seen = [], set()
        if self._table.selectionModel() is None:
            return result
        for idx in self._table.selectionModel().selectedRows():
            if idx.row() in seen:
                continue
            seen.add(idx.row())
            src = self._proxy.mapToSource(idx)
            f = self._model.file_at(src.row())
            if f is not None:
                result.append(f)
        return result

    def _on_table_activated(self, _index) -> None:
        self._open_selected()

    def _open_selected(self) -> None:
        files = self._selected_files()
        if not files:
            return
        missing = 0
        for f in files:
            if not os.path.exists(f.path):
                missing += 1
                continue
            if not QDesktopServices.openUrl(QUrl.fromLocalFile(f.path)):
                missing += 1
        if missing:
            self.status_bar.showMessage(f"Не удалось открыть файлов: {missing}", 4000)

    def _reveal_selected(self) -> None:
        files = self._selected_files()
        if files:
            open_in_explorer(files[0].path)

    def _copy_selected_paths(self) -> None:
        files = self._selected_files()
        if not files:
            return
        QApplication.clipboard().setText("\n".join(f.path for f in files))
        self.status_bar.showMessage(f"Скопировано путей: {len(files)}", 3000)

    def _show_table_context_menu(self, pos: QPoint) -> None:
        index = self._table.indexAt(pos)
        if not index.isValid():
            return
        if not self._table.selectionModel().isSelected(index):
            self._table.selectRow(index.row())
        menu = QMenu(self)
        act_open = menu.addAction("Открыть")
        act_reveal = menu.addAction("Показать в проводнике")
        act_copy = menu.addAction("Копировать путь")
        menu.addSeparator()
        verb = "Удалить в корзину…" if HAS_SEND2TRASH else "Удалить безвозвратно…"
        act_delete = menu.addAction(verb)
        chosen = menu.exec(self._table.viewport().mapToGlobal(pos))
        if chosen == act_open:
            self._open_selected()
        elif chosen == act_reveal:
            self._reveal_selected()
        elif chosen == act_copy:
            self._copy_selected_paths()
        elif chosen == act_delete:
            self._delete_selected_files()

    def _delete_selected_files(self) -> None:
        files = self._selected_files()
        if not files:
            return
        # Безопасный режим: если включён — всегда в корзину (когда доступна)
        safe_mode = bool(SETTINGS["safe_mode"])
        use_trash = HAS_SEND2TRASH or safe_mode
        if safe_mode and not HAS_SEND2TRASH:
            QMessageBox.warning(
                self, "Безопасный режим",
                "Безопасный режим включён, но send2trash не установлен.\n"
                "Удаление безвозвратное. Отключите безопасный режим "
                "или установите send2trash.")
            return

        # Проверяем каждый файл на защиту
        protected_files: List[Tuple[FileInfo, str]] = []
        for f in files:
            # Проверка относительно корня сканирования, если он известен
            root = self._root_display or os.path.dirname(f.path)
            valid, reason = validate_delete_path(f.path, root)
            if not valid:
                protected_files.append((f, reason))

        allowed = [f for f in files if f not in [pf[0] for pf in protected_files]]

        if protected_files:
            details = "\n".join(f"• {f.name}: {r}" for f, r in protected_files[:10])
            more = (f"\n… и ещё {len(protected_files) - 10}"
                    if len(protected_files) > 10 else "")
            QMessageBox.warning(
                self, "Защищённые файлы",
                f"Следующие файлы будут пропущены (защита):\n\n{details}{more}")
        if not allowed:
            return

        verb = ("переместить в корзину" if use_trash
                else "БЕЗВОЗВРАТНО удалить")
        names = "\n".join(f.name for f in allowed[:10])
        more = f"\n… и ещё {len(allowed) - 10}" if len(allowed) > 10 else ""
        answer = QMessageBox.question(
            self, "Подтверждение удаления",
            f"Вы уверены, что хотите {verb} {len(allowed)} файл(ов)?\n\n"
            f"{names}{more}",
            QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No,
            QMessageBox.StandardButton.No,
        )
        if answer != QMessageBox.StandardButton.Yes:
            return

        deleted, errors = [], []
        for f in allowed:
            try:
                # Финальная проверка защиты непосредственно перед удалением
                ok, reason = is_protected(f.path)
                if ok:
                    errors.append(f"{f.name}: {reason}")
                    continue
                if use_trash and HAS_SEND2TRASH:
                    _send2trash(f.path)
                else:
                    _make_writable(f.path)
                    os.remove(f.path)
                deleted.append(f)
            except Exception as exc:
                errors.append(f"{f.name}: {exc}")
        if deleted:
            self._remove_files_from_model(deleted)
        if errors:
            QMessageBox.warning(self, "Некоторые файлы не удалены",
                                "Не удалось удалить:\n" + "\n".join(errors[:15]))

    def _remove_files_from_model(self, deleted: List[FileInfo]) -> None:
        norm_set = {f.normalized_path for f in deleted}
        self._model.remove_files(norm_set)
        touched: Set[str] = set()
        for f in deleted:
            key = normalize_path(f.dir_path)
            safety = 0
            while key and key in self._folder_stats and safety < 10000:
                stats = self._folder_stats[key]
                stats.count = max(0, stats.count - 1)
                stats.size_bytes = max(0, stats.size_bytes - f.size)
                touched.add(key)
                next_key = stats.parent_key
                if not next_key or next_key == key:
                    break
                key = next_key
                safety += 1
            cstats = self._category_stats.get(f.category)
            if cstats:
                cstats.count = max(0, cstats.count - 1)
                cstats.size_bytes = max(0, cstats.size_bytes - f.size)
            self._total_bytes = max(0, self._total_bytes - f.size)
        for key in touched:
            item = self._tree_nodes.get(key)
            stats = self._folder_stats.get(key)
            if item and stats:
                item.setText(1, str(stats.count))
                item.setText(2, human_size(stats.size_bytes))
        self._refresh_category_table()
        self._update_statusbar()

    # -------------------------------------------------------------- Очистка
    def _build_cleanup_tab(self) -> None:
        tab = QWidget()
        layout = QVBoxLayout(tab)
        layout.setContentsMargins(12, 12, 12, 12)

        title = QLabel("Очистка системы")
        title.setStyleSheet("font-size: 20px; font-weight: 800;")
        layout.addWidget(title)

        cat_box = QGroupBox("Категории")
        cat_grid = QGridLayout(cat_box)
        self._cleanup_checks: Dict[str, QCheckBox] = {}
        cats = [("temp", "Временные файлы"), ("thumbnails", "Кэш миниатюр"),
                ("privacy", "Кэши браузеров"), ("games", "Игровые кэши"),
                ("recent", "Недавние документы")]
        for i, (key, txt) in enumerate(cats):
            cb = QCheckBox(txt)
            cb.setChecked(True)
            self._cleanup_checks[key] = cb
            cat_grid.addWidget(cb, i // 2, i % 2)

        cat_grid.addWidget(QLabel("Удалять файлы старше:"), 3, 0)
        self._cleanup_age = QSpinBox()
        self._cleanup_age.setRange(0, 24 * 365)
        self._cleanup_age.setSuffix(" ч")
        self._cleanup_age.setValue(int(SETTINGS["min_age_hours"]))
        cat_grid.addWidget(self._cleanup_age, 3, 1)
        layout.addWidget(cat_box)

        row = QHBoxLayout()
        self._scan_cleanup_btn = QPushButton("🔍 Найти файлы")
        self._scan_cleanup_btn.clicked.connect(self._request_cleanup_scan)
        self._delete_cleanup_btn = QPushButton("🗑 Удалить отмеченное")
        self._delete_cleanup_btn.setEnabled(False)
        self._delete_cleanup_btn.clicked.connect(self._request_cleanup_delete)
        export_btn = QPushButton("💾 Экспорт CSV")
        export_btn.clicked.connect(self._export_cleanup)
        recycle_btn = QPushButton("🗑 Очистить корзину")
        recycle_btn.clicked.connect(self._request_recycle_cleanup)
        dns_btn = QPushButton("🌐 Очистить DNS")
        dns_btn.clicked.connect(lambda: self._start_worker("dns"))
        row.addWidget(self._scan_cleanup_btn)
        row.addWidget(self._delete_cleanup_btn)
        row.addWidget(export_btn)
        row.addWidget(recycle_btn)
        row.addWidget(dns_btn)
        row.addStretch()
        layout.addLayout(row)

        self._cleanup_summary = QLabel("Предварительный просмотр не выполнен")
        self._cleanup_summary.setWordWrap(True)
        layout.addWidget(self._cleanup_summary)

        self._cleanup_tree = QTreeWidget()
        self._cleanup_tree.setAlternatingRowColors(True)
        self._cleanup_tree.setHeaderLabels(
            ["Удалить", "Категория", "Путь", "Размер", "Изменён"])
        for i, mode in enumerate([
            QHeaderView.ResizeMode.ResizeToContents,
            QHeaderView.ResizeMode.ResizeToContents,
            QHeaderView.ResizeMode.Stretch,
            QHeaderView.ResizeMode.ResizeToContents,
            QHeaderView.ResizeMode.ResizeToContents,
        ]):
            self._cleanup_tree.header().setSectionResizeMode(i, mode)
        self._cleanup_tree.itemDoubleClicked.connect(self._open_cleanup_item)
        layout.addWidget(self._cleanup_tree)

        self.tab_cleanup = tab
        self.tabs.addTab(tab, "Очистка")

    def _request_cleanup_scan(self) -> None:
        cats = [k for k, cb in self._cleanup_checks.items() if cb.isChecked()]
        if not cats:
            QMessageBox.information(self, "Очистка", "Выберите хотя бы одну категорию.")
            return
        age = self._cleanup_age.value()
        if age == 0:
            ans = QMessageBox.warning(
                self, "Возраст = 0",
                "Будут показаны ВСЕ файлы выбранных категорий, включая свежие.\n"
                "Продолжить?",
                QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No,
                QMessageBox.StandardButton.No,
            )
            if ans != QMessageBox.StandardButton.Yes:
                return
        self._cleanup_tree.clear()
        self._preview_candidates = []
        self._delete_cleanup_btn.setEnabled(False)
        self._start_worker("cleanup_scan", {
            "categories": cats, "age": age,
        })

    def _show_cleanup_preview(self, candidates: List[Candidate]) -> None:
        self._preview_candidates = list(candidates)
        self._cleanup_tree.clear()
        total = 0
        for i, c in enumerate(self._preview_candidates):
            total += c.size
            item = QTreeWidgetItem([
                "", CATEGORY_NAMES_RU.get(c.category, c.category),
                c.path, human_size(c.size), format_dt(c.modified),
            ])
            item.setFlags(item.flags() | Qt.ItemFlag.ItemIsUserCheckable)
            item.setCheckState(0, Qt.CheckState.Checked)
            item.setData(0, Qt.ItemDataRole.UserRole, i)
            item.setData(2, Qt.ItemDataRole.UserRole, c.path)
            self._cleanup_tree.addTopLevelItem(item)
        self._cleanup_summary.setText(
            f"Найдено файлов: {len(candidates)}; размер: {human_size(total)}")
        self._delete_cleanup_btn.setEnabled(bool(candidates))

    def _checked_cleanup_candidates(self) -> List[Candidate]:
        out = []
        for row in range(self._cleanup_tree.topLevelItemCount()):
            item = self._cleanup_tree.topLevelItem(row)
            if item.checkState(0) != Qt.CheckState.Checked:
                continue
            idx = item.data(0, Qt.ItemDataRole.UserRole)
            if isinstance(idx, int) and 0 <= idx < len(self._preview_candidates):
                out.append(self._preview_candidates[idx])
        return out

    def _request_cleanup_delete(self) -> None:
        candidates = self._checked_cleanup_candidates()
        if not candidates:
            QMessageBox.information(self, "Очистка", "Нет отмеченных файлов.")
            return
        total = sum(c.size for c in candidates)
        ans = QMessageBox.warning(
            self, "Подтверждение",
            f"Будет удалено файлов: {len(candidates)}\n"
            f"Размер: {human_size(total)}\n\n"
            "Файлы удаляются без помещения в корзину.\nПродолжить?",
            QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No,
            QMessageBox.StandardButton.No,
        )
        if ans != QMessageBox.StandardButton.Yes:
            return
        self._start_worker("cleanup_delete", {"candidates": candidates})

    def _show_cleanup_result(self, result: OperationResult) -> None:
        report = save_result_report("cleanup", result)
        SETTINGS["last_cleanup"] = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        SETTINGS["last_freed"] = result.bytes_freed
        SETTINGS.save()

        msg = (f"Статус: {result.status}\n"
               f"Удалено: {result.deleted}\n"
               f"Освобождено: {human_size(result.bytes_freed)}\n"
               f"Пропущено: {result.skipped}\n"
               f"Заблокировано: {result.locked}\n"
               f"Нет доступа: {result.access_denied}\n"
               f"Ошибок: {result.errors}\n\n"
               f"Отчёт:\n{report}")
        if result.status == "success":
            QMessageBox.information(self, "Очистка завершена", msg)
        else:
            QMessageBox.warning(self, "Очистка завершена", msg)

        self._preview_candidates = []
        self._cleanup_tree.clear()
        self._delete_cleanup_btn.setEnabled(False)
        self._cleanup_summary.setText("Выполните новый поиск для обновления списка.")

    def _open_cleanup_item(self, item, _column) -> None:
        path = item.data(2, Qt.ItemDataRole.UserRole)
        if path:
            open_in_explorer(path)

    def _export_cleanup(self) -> None:
        if not self._preview_candidates:
            return
        default = os.path.join(get_desktop(), "cleanup_preview.csv")
        path, _ = QFileDialog.getSaveFileName(self, "Экспорт", default, "CSV (*.csv)")
        if not path:
            return
        if not path.lower().endswith(".csv"):
            path += ".csv"
        try:
            with open(path, "w", encoding="utf-8-sig", newline="") as fh:
                w = csv.writer(fh, delimiter=";")
                w.writerow(["Категория", "Путь", "Размер", "Изменён"])
                for c in self._preview_candidates:
                    w.writerow([CATEGORY_NAMES_RU.get(c.category, c.category),
                                c.path, c.size, format_dt(c.modified)])
            QMessageBox.information(self, "Экспорт", f"Сохранено:\n{path}")
        except Exception as exc:
            QMessageBox.warning(self, "Ошибка", str(exc))

    def _request_recycle_cleanup(self) -> None:
        size, items = recycle_size()
        ans = QMessageBox.question(
            self, "Очистка корзины",
            f"Приблизительный размер корзины: {human_size(size)}\n"
            f"Элементов: {items}\n\nОчистить?",
            QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No,
        )
        if ans == QMessageBox.StandardButton.Yes:
            self._start_worker("recycle")

    # ----------------------------------------------------------- Анализ папок
    def _build_folder_size_tab(self) -> None:
        tab = QWidget()
        layout = QVBoxLayout(tab)
        layout.setContentsMargins(12, 12, 12, 12)

        title = QLabel("Анализ размеров папок")
        title.setStyleSheet("font-size: 20px; font-weight: 800;")
        layout.addWidget(title)

        row = QHBoxLayout()
        self._fs_path_edit = QLineEdit()
        self._fs_path_edit.setPlaceholderText("Путь к папке…")
        self._fs_path_edit.returnPressed.connect(self._start_fs_scan)
        pick = QPushButton("📂 Выбрать")
        pick.clicked.connect(self._pick_fs_folder)
        go = QPushButton("📊 Анализировать")
        go.clicked.connect(self._start_fs_scan)
        row.addWidget(QLabel("Путь:"))
        row.addWidget(self._fs_path_edit, 1)
        row.addWidget(pick)
        row.addWidget(go)
        layout.addLayout(row)

        quick = QHBoxLayout()
        quick.addWidget(QLabel("Быстро:"))
        home = os.path.expanduser("~")
        quick_paths = [("🏠 Дом", home)]
        if not IS_WINDOWS:
            quick_paths.append(("/", "/"))
        else:
            for d in ("C:\\", "C:\\Users", "C:\\ProgramData"):
                if os.path.exists(d):
                    quick_paths.append((d, d))
        for label, p in quick_paths:
            btn = QPushButton(label)
            btn.clicked.connect(lambda _=False, pp=p: self._fs_path_edit.setText(pp))
            quick.addWidget(btn)
        quick.addStretch()
        layout.addLayout(quick)

        self._fs_tree = QTreeWidget()
        self._fs_tree.setAlternatingRowColors(True)
        self._fs_tree.setHeaderLabels(
            ["Папка", "Размер", "% от общего", "Файлов", "Диаграмма"])
        for i in (0, 4):
            self._fs_tree.header().setSectionResizeMode(
                i, QHeaderView.ResizeMode.Stretch)
        for i in (1, 2, 3):
            self._fs_tree.header().setSectionResizeMode(
                i, QHeaderView.ResizeMode.ResizeToContents)
        self._fs_tree.itemDoubleClicked.connect(self._on_fs_double_click)
        layout.addWidget(self._fs_tree)

        bottom = QHBoxLayout()
        self._fs_summary = QLabel("—")
        bottom.addWidget(self._fs_summary, 1)
        exp = QPushButton("💾 Экспорт")
        exp.clicked.connect(self._export_fs)
        bottom.addWidget(exp)
        layout.addLayout(bottom)

        self.tab_fs = tab
        self.tabs.addTab(tab, "Анализ папок")

    def _pick_fs_folder(self) -> None:
        start = self._fs_path_edit.text().strip() or os.path.expanduser("~")
        path = QFileDialog.getExistingDirectory(self, "Выберите папку", start)
        if path:
            self._fs_path_edit.setText(path)

    def _start_fs_scan(self) -> None:
        if self._is_busy():
            return
        root = self._fs_path_edit.text().strip().strip('"').strip("'")
        if not root:
            QMessageBox.information(self, "Инфо", "Укажите путь к папке.")
            return
        root = os.path.abspath(os.path.expanduser(root))
        if not os.path.isdir(root):
            QMessageBox.critical(self, "Ошибка", f"Папка не найдена:\n{root}")
            return
        self._fs_tree.clear()
        self._fs_root = root
        self._fs_items = []
        self._fs_summary.setText("⏳ Сканирую…")
        self._start_worker("folder_sizes", {"root": root})

    def _show_fs_results(self, items: List[FolderSize]) -> None:
        self._fs_items = items
        self._fs_tree.clear()
        if not items:
            self._fs_summary.setText("Подпапок не найдено.")
            return
        total = sum(x.size for x in items)
        max_size = max((x.size for x in items), default=1) or 1
        for it in items:
            pct = it.size / total * 100 if total else 0
            bar_len = 30
            filled = int(round(bar_len * (it.size / max_size))) if max_size else 0
            bar = "█" * filled + "░" * (bar_len - filled)
            tree_item = QTreeWidgetItem([
                f"📁 {it.name}", human_size(it.size),
                f"{pct:.1f}%", f"{it.files:,}".replace(",", " "), bar,
            ])
            tree_item.setData(0, Qt.ItemDataRole.UserRole, it.path)
            self._fs_tree.addTopLevelItem(tree_item)
        self._fs_summary.setText(
            f"Папок: {len(items)} | Общий объём: {human_size(total)} | "
            f"Путь: {self._fs_root}"
        )

    def _on_fs_double_click(self, item, _col) -> None:
        path = item.data(0, Qt.ItemDataRole.UserRole)
        if path and os.path.isdir(path):
            open_in_explorer(path)

    def _export_fs(self) -> None:
        if not self._fs_items:
            QMessageBox.information(self, "Инфо", "Сначала выполните анализ.")
            return
        path, _ = QFileDialog.getSaveFileName(
            self, "Экспорт", os.path.join(get_desktop(), "folder_sizes.txt"),
            "Text (*.txt);;All (*.*)")
        if not path:
            return
        try:
            total = sum(x.size for x in self._fs_items)
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(f"Анализ размеров папок — {datetime.now()}\n")
                fh.write(f"Корень: {self._fs_root}\n")
                fh.write("=" * 70 + "\n\n")
                for it in self._fs_items:
                    pct = it.size / total * 100 if total else 0
                    fh.write(f"{human_size(it.size):>12}  {pct:>5.1f}%  "
                             f"{it.files:>8} файлов  {it.path}\n")
                fh.write(f"\nИтого: {human_size(total)}\n")
            QMessageBox.information(self, "Готово", f"Сохранено:\n{path}")
        except Exception as exc:
            QMessageBox.warning(self, "Ошибка", str(exc))

    # ------------------------------------------------------------ Дубликаты
    def _build_duplicates_tab(self) -> None:
        tab = QWidget()
        layout = QVBoxLayout(tab)
        layout.setContentsMargins(12, 12, 12, 12)

        title = QLabel("Поиск дубликатов")
        title.setStyleSheet("font-size: 20px; font-weight: 800;")
        layout.addWidget(title)

        row = QHBoxLayout()
        self._dup_roots_edit = QLineEdit(get_user_profile())
        self._dup_roots_edit.setPlaceholderText("Папки через точку с запятой")
        add = QPushButton("📂 Добавить папку")
        add.clicked.connect(lambda: self._add_root_to(self._dup_roots_edit))
        row.addWidget(QLabel("Папки:"))
        row.addWidget(self._dup_roots_edit, 1)
        row.addWidget(add)
        layout.addLayout(row)

        opts = QHBoxLayout()
        opts.addWidget(QLabel("Мин. размер (МБ):"))
        self._dup_min_mb = QSpinBox()
        self._dup_min_mb.setRange(1, 100000)
        self._dup_min_mb.setValue(int(SETTINGS["duplicate_mb"]))
        opts.addWidget(self._dup_min_mb)
        go = QPushButton("🔍 Найти дубликаты")
        go.clicked.connect(self._start_dup_scan)
        opts.addWidget(go)
        opts.addStretch()
        layout.addLayout(opts)

        self._dup_tree = QTreeWidget()
        self._dup_tree.setAlternatingRowColors(True)
        self._dup_tree.setHeaderLabels(["Группа / Путь", "Размер", "Доп."])
        self._dup_tree.header().setSectionResizeMode(
            0, QHeaderView.ResizeMode.Stretch)
        for i in (1, 2):
            self._dup_tree.header().setSectionResizeMode(
                i, QHeaderView.ResizeMode.ResizeToContents)
        self._dup_tree.itemDoubleClicked.connect(self._on_dup_double_click)
        layout.addWidget(self._dup_tree)

        bottom = QHBoxLayout()
        self._dup_summary = QLabel("—")
        bottom.addWidget(self._dup_summary, 1)
        exp = QPushButton("💾 Экспорт")
        exp.clicked.connect(self._export_dup)
        bottom.addWidget(exp)
        layout.addLayout(bottom)

        self.tab_dup = tab
        self.tabs.addTab(tab, "Дубликаты")

    def _add_root_to(self, edit: QLineEdit) -> None:
        folder = QFileDialog.getExistingDirectory(
            self, "Выберите папку", get_user_profile())
        if not folder:
            return
        current = edit.text().strip()
        parts = ([p.strip() for p in current.split(";") if p.strip()]
                 if current else [])
        if folder not in parts:
            parts.append(folder)
        edit.setText("; ".join(parts))

    def _parse_roots(self, text: str) -> List[str]:
        parts = re.split(r"[;\n]+", text)
        return [normalize_path(p) for p in parts
                if p.strip() and os.path.isdir(p.strip())]

    def _start_dup_scan(self) -> None:
        if self._is_busy():
            return
        roots = self._parse_roots(self._dup_roots_edit.text())
        if not roots:
            QMessageBox.information(self, "Инфо",
                                    "Укажите хотя бы одну существующую папку.")
            return
        self._dup_tree.clear()
        self._start_worker("duplicates",
                           {"roots": roots, "min_mb": self._dup_min_mb.value()})

    def _show_duplicates(self, groups: List[Dict[str, Any]]) -> None:
        self._dup_groups = groups
        self._dup_tree.clear()
        if not groups:
            self._dup_tree.addTopLevelItem(
                QTreeWidgetItem(["Дубликаты не найдены", "", ""]))
            self._dup_summary.setText("Групп: 0")
            return
        for n, g in enumerate(groups, 1):
            parent = QTreeWidgetItem([
                f"Группа {n}: {len(g['files'])} файлов",
                human_size(g["size"]),
                f"Лишних: {human_size(g['wasted'])}",
            ])
            for path in g["files"]:
                child = QTreeWidgetItem(
                    [path, human_size(g["size"]), g["hash"][:16] + "…"])
                child.setData(0, Qt.ItemDataRole.UserRole, path)
                parent.addChild(child)
            parent.setExpanded(True)
            self._dup_tree.addTopLevelItem(parent)
        total_dup = sum(len(g["files"]) - 1 for g in groups)
        wasted = sum(g["wasted"] for g in groups)
        self._dup_summary.setText(
            f"Групп: {len(groups)} | Лишних копий: {total_dup} | "
            f"Можно освободить: {human_size(wasted)}")

    def _on_dup_double_click(self, item, _col) -> None:
        path = item.data(0, Qt.ItemDataRole.UserRole)
        if path:
            open_in_explorer(path)

    def _export_dup(self) -> None:
        if not self._dup_groups:
            return
        path, _ = QFileDialog.getSaveFileName(
            self, "Экспорт", os.path.join(get_desktop(), "duplicates.txt"),
            "Text (*.txt)")
        if not path:
            return
        try:
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(f"Отчёт о дубликатах — {datetime.now()}\n"
                         + "=" * 60 + "\n\n")
                for n, g in enumerate(self._dup_groups, 1):
                    fh.write(f"#{n}  hash={g['hash']}  "
                             f"размер={human_size(g['size'])}\n")
                    for p in g["files"]:
                        fh.write(f"  {p}\n")
                    fh.write("\n")
            QMessageBox.information(self, "Готово", f"Сохранено:\n{path}")
        except Exception as exc:
            QMessageBox.warning(self, "Ошибка", str(exc))

    # --------------------------------------------------------- Большие файлы
    def _build_large_tab(self) -> None:
        tab = QWidget()
        layout = QVBoxLayout(tab)
        layout.setContentsMargins(12, 12, 12, 12)

        title = QLabel("Большие файлы")
        title.setStyleSheet("font-size: 20px; font-weight: 800;")
        layout.addWidget(title)

        row = QHBoxLayout()
        self._large_roots_edit = QLineEdit(get_user_profile())
        self._large_roots_edit.setPlaceholderText("Папки через точку с запятой")
        add = QPushButton("📂 Добавить папку")
        add.clicked.connect(lambda: self._add_root_to(self._large_roots_edit))
        row.addWidget(QLabel("Папки:"))
        row.addWidget(self._large_roots_edit, 1)
        row.addWidget(add)
        layout.addLayout(row)

        opts = QHBoxLayout()
        opts.addWidget(QLabel("Мин. размер (МБ):"))
        self._large_min_mb = QSpinBox()
        self._large_min_mb.setRange(1, 1000000)
        self._large_min_mb.setValue(int(SETTINGS["big_file_mb"]))
        opts.addWidget(self._large_min_mb)
        go = QPushButton("🔍 Найти большие файлы")
        go.clicked.connect(self._start_large_scan)
        opts.addWidget(go)
        opts.addStretch()
        layout.addLayout(opts)

        self._large_tree = QTreeWidget()
        self._large_tree.setAlternatingRowColors(True)
        self._large_tree.setHeaderLabels(["Путь", "Размер", "Изменён"])
        self._large_tree.header().setSectionResizeMode(
            0, QHeaderView.ResizeMode.Stretch)
        for i in (1, 2):
            self._large_tree.header().setSectionResizeMode(
                i, QHeaderView.ResizeMode.ResizeToContents)
        self._large_tree.itemDoubleClicked.connect(
            lambda it, _c: open_in_explorer(
                it.data(0, Qt.ItemDataRole.UserRole) or ""))
        layout.addWidget(self._large_tree)

        bottom = QHBoxLayout()
        self._large_summary = QLabel("—")
        bottom.addWidget(self._large_summary, 1)
        exp = QPushButton("💾 Экспорт")
        exp.clicked.connect(self._export_large)
        bottom.addWidget(exp)
        layout.addLayout(bottom)

        self.tab_large = tab
        self.tabs.addTab(tab, "Большие файлы")

    def _start_large_scan(self) -> None:
        if self._is_busy():
            return
        roots = self._parse_roots(self._large_roots_edit.text())
        if not roots:
            QMessageBox.information(self, "Инфо",
                                    "Укажите хотя бы одну существующую папку.")
            return
        self._large_tree.clear()
        self._start_worker("big_files",
                           {"roots": roots, "min_mb": self._large_min_mb.value()})

    def _show_large(self, items: List[Dict[str, Any]]) -> None:
        self._large_items = items
        self._large_tree.clear()
        for row in items[:500]:
            it = QTreeWidgetItem([row["path"], human_size(row["size"]),
                                  format_dt(row["modified"])])
            it.setData(0, Qt.ItemDataRole.UserRole, row["path"])
            self._large_tree.addTopLevelItem(it)
        total = sum(i["size"] for i in items)
        extra = f" (+ ещё {len(items) - 500})" if len(items) > 500 else ""
        self._large_summary.setText(
            f"Найдено: {len(items)}{extra} файлов | Всего: {human_size(total)}")

    def _export_large(self) -> None:
        if not self._large_items:
            return
        path, _ = QFileDialog.getSaveFileName(
            self, "Экспорт", os.path.join(get_desktop(), "large_files.txt"),
            "Text (*.txt)")
        if not path:
            return
        try:
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(f"Большие файлы — {datetime.now()}\n" + "=" * 60 + "\n\n")
                for it in self._large_items:
                    fh.write(f"{human_size(it['size']):>12}  {it['path']}\n")
            QMessageBox.information(self, "Готово", f"Сохранено:\n{path}")
        except Exception as exc:
            QMessageBox.warning(self, "Ошибка", str(exc))

    # ------------------------------------------------------------- Процессы
    def _build_processes_tab(self) -> None:
        tab = QWidget()
        layout = QVBoxLayout(tab)
        layout.setContentsMargins(12, 12, 12, 12)
        title = QLabel("Процессы")
        title.setStyleSheet("font-size: 20px; font-weight: 800;")
        layout.addWidget(title)

        row = QHBoxLayout()
        refresh = QPushButton("🔄 Обновить")
        refresh.clicked.connect(lambda: self._start_worker("processes"))
        term = QPushButton("🛑 Завершить выбранный")
        term.clicked.connect(self._terminate_selected_process)
        row.addWidget(refresh)
        row.addWidget(term)
        row.addStretch()
        layout.addLayout(row)

        self._proc_tree = QTreeWidget()
        self._proc_tree.setAlternatingRowColors(True)
        self._proc_tree.setHeaderLabels(
            ["PID", "Имя", "Память", "Статус", "Пользователь"])
        for i, mode in enumerate([
            QHeaderView.ResizeMode.ResizeToContents,
            QHeaderView.ResizeMode.Stretch,
            QHeaderView.ResizeMode.ResizeToContents,
            QHeaderView.ResizeMode.ResizeToContents,
            QHeaderView.ResizeMode.Stretch,
        ]):
            self._proc_tree.header().setSectionResizeMode(i, mode)
        layout.addWidget(self._proc_tree)

        self.tab_processes = tab
        self.tabs.addTab(tab, "Процессы")

    def _show_processes(self, procs: List[Dict[str, Any]]) -> None:
        self._proc_tree.clear()
        if not procs:
            self._proc_tree.addTopLevelItem(
                QTreeWidgetItem(["—", "psutil не установлен", "", "", ""]))
            return
        for p in procs:
            it = QTreeWidgetItem([
                str(p.get("pid", 0)), p.get("name", ""),
                human_size(p.get("memory", 0)), p.get("status", ""),
                p.get("user", ""),
            ])
            it.setData(0, Qt.ItemDataRole.UserRole, p.get("pid"))
            self._proc_tree.addTopLevelItem(it)

    def _terminate_selected_process(self) -> None:
        item = self._proc_tree.currentItem()
        if item is None:
            QMessageBox.information(self, "Процессы", "Выберите процесс.")
            return
        pid = item.data(0, Qt.ItemDataRole.UserRole)
        name = item.text(1)
        ans = QMessageBox.warning(
            self, "Завершение процесса",
            f"Завершить процесс?\n\n{name} (PID {pid})",
            QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No,
            QMessageBox.StandardButton.No,
        )
        if ans != QMessageBox.StandardButton.Yes:
            return
        ok, msg = terminate_process(pid)
        if ok:
            QMessageBox.information(self, "Процессы", msg)
            QTimer.singleShot(100, lambda: self._start_worker("processes"))
        else:
            QMessageBox.warning(self, "Ошибка", msg)

    # ----------------------------------------------------------- Автозагрузка
    def _build_startup_tab(self) -> None:
        tab = QWidget()
        layout = QVBoxLayout(tab)
        layout.setContentsMargins(12, 12, 12, 12)
        title = QLabel("Автозагрузка")
        title.setStyleSheet("font-size: 20px; font-weight: 800;")
        layout.addWidget(title)

        row = QHBoxLayout()
        refresh = QPushButton("🔄 Обновить")
        refresh.clicked.connect(lambda: self._start_worker("startup"))
        dis = QPushButton("Отключить выбранное")
        dis.clicked.connect(self._disable_selected_startup)
        rest = QPushButton("Восстановить из бэкапа")
        rest.clicked.connect(self._restore_startup_backup)
        row.addWidget(refresh)
        row.addWidget(dis)
        row.addWidget(rest)
        row.addStretch()
        layout.addLayout(row)

        self._startup_tree = QTreeWidget()
        self._startup_tree.setAlternatingRowColors(True)
        self._startup_tree.setHeaderLabels(
            ["Тип", "Имя", "Источник", "Команда / Путь"])
        for i, mode in enumerate([
            QHeaderView.ResizeMode.ResizeToContents,
            QHeaderView.ResizeMode.ResizeToContents,
            QHeaderView.ResizeMode.Stretch,
            QHeaderView.ResizeMode.Stretch,
        ]):
            self._startup_tree.header().setSectionResizeMode(i, mode)
        layout.addWidget(self._startup_tree)

        self.tab_startup = tab
        self.tabs.addTab(tab, "Автозагрузка")

    def _show_startup(self, items: List[Dict[str, Any]]) -> None:
        self._startup_tree.clear()
        if not items:
            self._startup_tree.addTopLevelItem(
                QTreeWidgetItem(["—", "Элементы не найдены", "", ""]))
            return
        for data in items:
            it = QTreeWidgetItem([
                "Реестр" if data.get("kind") == "registry" else "Файл",
                str(data.get("name", "")),
                str(data.get("source", "")),
                str(data.get("value", "")),
            ])
            it.setData(0, Qt.ItemDataRole.UserRole, data)
            self._startup_tree.addTopLevelItem(it)

    def _disable_selected_startup(self) -> None:
        item = self._startup_tree.currentItem()
        if item is None:
            QMessageBox.information(self, "Автозагрузка", "Выберите элемент.")
            return
        data = item.data(0, Qt.ItemDataRole.UserRole)
        if not isinstance(data, dict):
            return
        ans = QMessageBox.question(
            self, "Автозагрузка",
            f"Отключить?\n\n{data.get('name', '')}\n{data.get('value', '')}",
            QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No,
        )
        if ans != QMessageBox.StandardButton.Yes:
            return
        ok, msg = disable_startup(data)
        if ok:
            QMessageBox.information(self, "Автозагрузка", msg)
            QTimer.singleShot(100, lambda: self._start_worker("startup"))
        else:
            QMessageBox.warning(self, "Ошибка", msg)

    def _restore_startup_backup(self) -> None:
        backups = load_json(STARTUP_BACKUP_FILE, [])
        if not isinstance(backups, list) or not backups:
            QMessageBox.information(self, "Резервные копии",
                                    "Резервные копии отсутствуют.")
            return
        labels: List[str] = []
        id_by_label: Dict[str, str] = {}
        for b in backups:
            label = (f"{b.get('name', '—')} | {b.get('kind', '—')} | "
                     f"{b.get('created', '—')} | {b.get('id', '')}")
            labels.append(label)
            id_by_label[label] = b.get("id", "")
        selected, accepted = QInputDialog.getItem(
            self, "Восстановление автозагрузки", "Выберите:", labels, 0, False)
        if not accepted or not selected:
            return
        bid = id_by_label.get(selected, "")
        if not bid:
            return
        ok, msg = restore_startup(bid)
        if ok:
            QMessageBox.information(self, "Восстановление", msg)
            QTimer.singleShot(100, lambda: self._start_worker("startup"))
        else:
            QMessageBox.warning(self, "Ошибка", msg)

    # -------------------------------------------------------------- Настройки
    def _build_settings_tab(self) -> None:
        tab = QWidget()
        layout = QVBoxLayout(tab)
        layout.setContentsMargins(12, 12, 12, 12)
        title = QLabel("Настройки")
        title.setStyleSheet("font-size: 20px; font-weight: 800;")
        layout.addWidget(title)

        grid = QGridLayout()

        self._set_safe_mode = QCheckBox(
            "Безопасный режим (удаление через корзину, если доступно)")
        self._set_safe_mode.setChecked(bool(SETTINGS["safe_mode"]))
        grid.addWidget(self._set_safe_mode, 0, 0, 1, 2)

        grid.addWidget(QLabel("Удалять только старше N часов:"), 1, 0)
        self._set_age = QSpinBox()
        self._set_age.setRange(0, 24 * 365)
        self._set_age.setValue(int(SETTINGS["min_age_hours"]))
        grid.addWidget(self._set_age, 1, 1)

        grid.addWidget(QLabel("Большие файлы от (МБ):"), 2, 0)
        self._set_big = QSpinBox()
        self._set_big.setRange(1, 1000000)
        self._set_big.setValue(int(SETTINGS["big_file_mb"]))
        grid.addWidget(self._set_big, 2, 1)

        grid.addWidget(QLabel("Дубликаты от (МБ):"), 3, 0)
        self._set_dup = QSpinBox()
        self._set_dup.setRange(1, 1000000)
        self._set_dup.setValue(int(SETTINGS["duplicate_mb"]))
        grid.addWidget(self._set_dup, 3, 1)

        grid.addWidget(QLabel("Защищённые расширения:"), 4, 0)
        self._set_exts = QLineEdit(", ".join(SETTINGS["keep_extensions"]))
        grid.addWidget(self._set_exts, 4, 1)

        grid.addWidget(QLabel("Игнорируемые папки:"), 5, 0)
        self._set_ignore = QLineEdit(", ".join(SETTINGS["ignore_dirs"]))
        grid.addWidget(self._set_ignore, 5, 1)

        layout.addLayout(grid)

        prot_box = QGroupBox("Дополнительно защищённые папки")
        prot_layout = QVBoxLayout(prot_box)
        self._prot_list = QTreeWidget()
        self._prot_list.setHeaderLabels(["Путь"])
        self._prot_list.setFixedHeight(120)
        prot_layout.addWidget(self._prot_list)
        prot_btns = QHBoxLayout()
        add_p = QPushButton("➕ Добавить")
        add_p.clicked.connect(self._add_protected_folder)
        clear_p = QPushButton("🗑 Очистить")
        clear_p.clicked.connect(self._clear_protected_folders)
        prot_btns.addWidget(add_p)
        prot_btns.addWidget(clear_p)
        prot_btns.addStretch()
        prot_layout.addLayout(prot_btns)
        layout.addWidget(prot_box)
        self._refresh_prot_list()

        bottom = QHBoxLayout()
        save = QPushButton("💾 Сохранить настройки")
        save.clicked.connect(self._save_ui_settings)
        open_rep = QPushButton("📁 Открыть отчёты")
        open_rep.clicked.connect(lambda: open_in_explorer(str(REPORT_DIR)))
        open_log = QPushButton("📄 Открыть логи")
        open_log.clicked.connect(lambda: open_in_explorer(str(LOG_DIR)))
        bottom.addWidget(save)
        bottom.addWidget(open_rep)
        bottom.addWidget(open_log)
        bottom.addStretch()
        layout.addLayout(bottom)

        info = QLabel(
            "Программа не проходит через junction, symlink и другие reparse "
            "points; не удаляет системные файлы и критичные расширения. "
            f"Настройки хранятся в {CONFIG_FILE}."
        )
        info.setWordWrap(True)
        info.setStyleSheet("color: #6b7280; padding: 8px;")
        layout.addWidget(info)

        self.tab_settings = tab
        self.tabs.addTab(tab, "Настройки")

    def _add_protected_folder(self) -> None:
        folder = QFileDialog.getExistingDirectory(
            self, "Папка, которую НИКОГДА не удалять")
        if not folder:
            return
        current = list(SETTINGS["custom_protected"])
        if folder not in current:
            current.append(folder)
            SETTINGS["custom_protected"] = current
            self._refresh_prot_list()
            SETTINGS.save()

    def _clear_protected_folders(self) -> None:
        SETTINGS["custom_protected"] = []
        self._refresh_prot_list()
        SETTINGS.save()

    def _refresh_prot_list(self) -> None:
        self._prot_list.clear()
        items = SETTINGS["custom_protected"]
        for p in items:
            self._prot_list.addTopLevelItem(QTreeWidgetItem([p]))
        if not items:
            self._prot_list.addTopLevelItem(QTreeWidgetItem(["(пусто)"]))

    def _save_ui_settings(self) -> None:
        SETTINGS["safe_mode"] = self._set_safe_mode.isChecked()
        SETTINGS["min_age_hours"] = self._set_age.value()
        SETTINGS["big_file_mb"] = self._set_big.value()
        SETTINGS["duplicate_mb"] = self._set_dup.value()
        exts = [_normalize_extension(e)
                for e in self._set_exts.text().split(",") if e.strip()]
        exts = [e for e in exts if e]
        if not exts:
            QMessageBox.warning(
                self, "Ошибка",
                "Список защищённых расширений не может быть пустым.")
            return
        SETTINGS["keep_extensions"] = exts
        SETTINGS["ignore_dirs"] = [
            s.strip().lower()
            for s in self._set_ignore.text().split(",") if s.strip()
        ]
        if SETTINGS.save():
            QMessageBox.information(self, "Настройки", "Настройки сохранены.")
        else:
            QMessageBox.warning(self, "Ошибка", "Не удалось сохранить.")

    # -------------------------------------------------------- Общий воркер-API
    def _is_busy(self) -> bool:
        return ((self._worker is not None and self._worker.isRunning()) or
                (self._scan_worker is not None and self._scan_worker.isRunning()))

    def _start_worker(self, operation: str,
                      payload: Optional[Dict[str, Any]] = None) -> None:
        if self._is_busy():
            QMessageBox.information(self, "Операция выполняется",
                                    "Дождитесь завершения текущей операции.")
            return
        self._worker = WorkerThread(operation, payload or {}, self)
        self._worker.progress.connect(self._on_worker_progress)
        self._worker.completed.connect(self._on_worker_completed)
        self._worker.failed.connect(self._on_worker_failed)
        self._worker.finished.connect(self._on_worker_finished)

        self._op_label.setText("Операция запущена")
        self._op_progress.setRange(0, 0)
        self._cancel_btn.setEnabled(True)
        self.status_bar.showMessage("Операция…")
        self._worker.start()

    def _on_worker_progress(self, message: str) -> None:
        self._op_label.setText(message)
        self.status_bar.showMessage(message)

    def _on_worker_finished(self) -> None:
        thread = self._worker
        self._worker = None
        self._op_progress.setRange(0, 1)
        self._op_progress.setValue(0)
        self._op_label.setText("Готово")
        self._cancel_btn.setEnabled(False)
        if thread is not None:
            thread.deleteLater()

    def _on_worker_failed(self, operation: str, message: str) -> None:
        QMessageBox.critical(self, "Ошибка",
                             f"Операция «{operation}» завершилась ошибкой:\n\n"
                             f"{message}")
        self.status_bar.showMessage("Ошибка операции")

    def _on_worker_completed(self, operation: str, data: Any) -> None:
        if operation == "health":
            self._show_health(data)
        elif operation == "cleanup_scan":
            self._show_cleanup_preview(data)
        elif operation == "cleanup_delete":
            self._show_cleanup_result(data)
        elif operation == "folder_sizes":
            self._show_fs_results(data)
        elif operation == "duplicates":
            self._show_duplicates(data)
        elif operation == "big_files":
            self._show_large(data)
        elif operation == "processes":
            self._show_processes(data)
        elif operation == "startup":
            self._show_startup(data)
        elif operation == "recycle":
            self._show_special("Очистка корзины", data)
        elif operation == "dns":
            self._show_special("Очистка DNS", data)
        self.status_bar.showMessage("Готово")

    def _show_special(self, title: str, result: OperationResult) -> None:
        report = save_result_report(title, result)
        msg = (f"{result.message}\n\nСтатус: {result.status}\n"
               f"Освобождено: {human_size(result.bytes_freed)}\n"
               f"Ошибок: {result.errors}\n\nОтчёт:\n{report}")
        if result.status == "success":
            QMessageBox.information(self, title, msg)
        else:
            QMessageBox.warning(self, title, msg)

    def _cancel_everything(self) -> None:
        if self._scan_worker and self._scan_worker.isRunning():
            self._scan_worker.request_stop()
            self._disk_progress.setFormat("Остановка…")
        if self._worker and self._worker.isRunning():
            self._worker.cancel()
            self._op_label.setText("Отмена…")

    def _on_tab_changed(self, _index: int) -> None:
        w = self.tabs.currentWidget()
        if w is self.tab_dashboard and not self._is_busy():
            self._refresh_dashboard()
        elif w is self.tab_processes and not self._is_busy():
            self._start_worker("processes")
        elif w is self.tab_startup and not self._is_busy():
            self._start_worker("startup")

    # ------------------------------------------------------------- CSV экспорт
    def _export_csv(self) -> None:
        if self._model.rowCount() == 0:
            QMessageBox.information(self, "Экспорт", "Нет данных для экспорта.")
            return
        path, _ = QFileDialog.getSaveFileName(
            self, "Экспорт в CSV", "files.csv", "CSV (*.csv)")
        if not path:
            return
        try:
            with open(path, "w", newline="", encoding="utf-8-sig") as fh:
                w = csv.writer(fh)
                w.writerow(["Имя", "Путь", "Категория", "Размер (байт)",
                            "Размер", "Изменён"])
                for row in range(self._proxy.rowCount()):
                    src = self._proxy.mapToSource(self._proxy.index(row, 0))
                    f = self._model.file_at(src.row())
                    if f:
                        w.writerow([f.name, f.path, f.category, f.size,
                                    human_size(f.size), format_dt(f.mtime)])
            QMessageBox.information(self, "Экспорт завершён", f"Сохранено:\n{path}")
        except OSError as exc:
            QMessageBox.critical(self, "Ошибка экспорта", str(exc))

    # -------------------------------------------------------------- Закрытие
    def closeEvent(self, event) -> None:
        # Сохраняем настройки до любых проверок
        if hasattr(self, "_path_edit"):
            SETTINGS["last_scan_path"] = self._path_edit.text()
        self._save_geometry()
        SETTINGS.save()

        if self._scan_worker and self._scan_worker.isRunning():
            ans = QMessageBox.question(
                self, "Сканирование выполняется",
                "Сканирование не завершено. Остановить и выйти?",
                QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No,
                QMessageBox.StandardButton.No,
            )
            if ans != QMessageBox.StandardButton.Yes:
                event.ignore()
                return
            self._scan_worker.request_stop()
            if not self._scan_worker.wait(5000):
                log.warning("ScanWorker не завершился за 5 секунд")

        if self._worker and self._worker.isRunning():
            ans = QMessageBox.question(
                self, "Операция выполняется",
                "Отменить операцию и выйти?",
                QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No,
                QMessageBox.StandardButton.Yes,
            )
            if ans != QMessageBox.StandardButton.Yes:
                event.ignore()
                return
            self._worker.cancel()
            if not self._worker.wait(5000):
                log.warning("WorkerThread не завершился за 5 секунд")

        event.accept()


# ============================================================================
#  SINGLE INSTANCE
# ============================================================================
class SingleInstanceGuard:
    """Проверка единственного экземпляра через QSharedMemory + QLockFile."""

    def __init__(self, key: str = "PCOptimizerCombined") -> None:
        self._key = key
        self._shared = QSharedMemory(key)
        self._lock = QLockFile(str(APP_DATA_DIR / "app.lock"))
        self._lock.setStaleLockTime(0)

    def acquire(self) -> bool:
        # Сначала пытаемся создать shared memory
        if self._shared.attach():
            self._shared.detach()
        if not self._shared.create(1):
            return False
        if not self._lock.tryLock(100):
            self._shared.detach()
            return False
        return True

    def release(self) -> None:
        try:
            self._lock.unlock()
        except Exception:
            pass
        try:
            if self._shared.isAttached():
                self._shared.detach()
        except Exception:
            pass


# ============================================================================
#  ЗАПУСК
# ============================================================================
def main() -> int:
    app = QApplication(sys.argv)
    app.setApplicationName(APP_NAME)
    app.setApplicationVersion(APP_VERSION)
    app.setOrganizationName(ORG_NAME)

    guard = SingleInstanceGuard()
    if not guard.acquire():
        QMessageBox.warning(None, APP_NAME,
                            "Программа уже запущена.")
        return 1

    try:
        window = MainWindow()
        window.show()
        return app.exec()
    finally:
        guard.release()


if __name__ == "__main__":
    sys.exit(main())