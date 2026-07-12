# -*- coding: utf-8 -*-

import ctypes
import json
import logging
import os
import re
import stat
import sys
import threading

from dataclasses import dataclass, field, asdict
from logging.handlers import RotatingFileHandler
from typing import Any, Dict, List


APP_NAME = "PC Optimizer"
APP_VERSION = "3.1.0"
MUTEX_NAME = r"Local\PCOptimizer_3_1"
TASK_NAME = "PC Optimizer Safe Cleanup"

if getattr(sys, "frozen", False):
    BASE_DIR = os.path.dirname(sys.executable)
else:
    BASE_DIR = os.path.dirname(os.path.abspath(__file__))

LOCAL_APPDATA = os.environ.get("LOCALAPPDATA", BASE_DIR)
APP_DATA_DIR = os.path.join(LOCAL_APPDATA, "PCOptimizer")

CONFIG_DIR = os.path.join(APP_DATA_DIR, "config")
LOG_DIR = os.path.join(APP_DATA_DIR, "logs")
REPORT_DIR = os.path.join(APP_DATA_DIR, "reports")
STARTUP_BACKUP_DIR = os.path.join(APP_DATA_DIR, "startup_backups")

CONFIG_FILE = os.path.join(CONFIG_DIR, "settings.json")
STARTUP_BACKUP_FILE = os.path.join(
    CONFIG_DIR,
    "startup_backups.json",
)
LOG_FILE = os.path.join(LOG_DIR, "pc_optimizer.log")

for directory in (
    APP_DATA_DIR,
    CONFIG_DIR,
    LOG_DIR,
    REPORT_DIR,
    STARTUP_BACKUP_DIR,
):
    os.makedirs(directory, exist_ok=True)


DEFAULT_SETTINGS = {
    "min_age_hours": 72,
    "big_file_mb": 250,
    "duplicate_mb": 20,
    "last_cleanup": "",
    "last_freed": 0,
}


LOG_HANDLER = RotatingFileHandler(
    LOG_FILE,
    maxBytes=5 * 1024 * 1024,
    backupCount=3,
    encoding="utf-8",
)

logging.basicConfig(
    handlers=[LOG_HANDLER],
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
)


@dataclass
class Candidate:
    category: str
    path: str
    root: str
    size: int
    modified: float

    def to_dict(self):
        return asdict(self)


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

    def merge(self, other):
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


class CancelToken:
    def __init__(self):
        self._event = threading.Event()

    def cancel(self):
        self._event.set()

    def cancelled(self):
        return self._event.is_set()


class SingleInstance:
    ERROR_ALREADY_EXISTS = 183

    def __init__(self, name=MUTEX_NAME):
        self.handle = None
        self.already_exists = False

        if os.name != "nt":
            return

        self.handle = ctypes.windll.kernel32.CreateMutexW(
            None,
            False,
            name,
        )

        self.already_exists = (
            ctypes.windll.kernel32.GetLastError()
            == self.ERROR_ALREADY_EXISTS
        )

    def close(self):
        if self.handle:
            ctypes.windll.kernel32.CloseHandle(self.handle)
            self.handle = None


def load_json(path, default):
    try:
        if not os.path.isfile(path):
            return default

        with open(path, "r", encoding="utf-8") as file:
            return json.load(file)

    except Exception:
        logging.exception("Не удалось прочитать JSON: %s", path)
        return default


def save_json(path, data):
    temp_path = path + ".tmp"

    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)

        with open(
            temp_path,
            "w",
            encoding="utf-8",
            newline="\n",
        ) as file:
            json.dump(
                data,
                file,
                ensure_ascii=False,
                indent=2,
            )

        os.replace(temp_path, path)
        return True

    except Exception:
        logging.exception("Не удалось сохранить JSON: %s", path)

        try:
            if os.path.isfile(temp_path):
                os.remove(temp_path)
        except OSError:
            pass

        return False


def load_settings():
    settings = dict(DEFAULT_SETTINGS)
    data = load_json(CONFIG_FILE, {})

    if isinstance(data, dict):
        settings.update(data)

    return settings


def save_settings(settings):
    return save_json(CONFIG_FILE, settings)


def normalize_path(path):
    if not path:
        return ""

    path = str(path).strip().strip('"')
    path = os.path.expandvars(os.path.expanduser(path))
    return os.path.abspath(os.path.normpath(path))


def normalize_paths(paths):
    result = []
    seen = set()

    for path in paths:
        path = normalize_path(path)

        if not path:
            continue

        key = os.path.normcase(path)

        if key not in seen:
            seen.add(key)
            result.append(path)

    return result


def get_user_profile():
    return normalize_path(
        os.environ.get("USERPROFILE", os.path.expanduser("~"))
    )


def get_windows_dir():
    return normalize_path(
        os.environ.get("WINDIR", r"C:\Windows")
    )


def get_system_drive():
    return normalize_path(
        os.environ.get("SystemDrive", "C:") + "\\"
    )


def get_desktop():
    desktop = os.path.join(get_user_profile(), "Desktop")

    if os.path.isdir(desktop):
        return desktop

    return get_user_profile()


def format_size(size):
    try:
        value = float(size or 0)
    except (TypeError, ValueError):
        value = 0.0

    units = ("Б", "КБ", "МБ", "ГБ", "ТБ", "ПБ")

    for unit in units:
        if value < 1024 or unit == units[-1]:
            return f"{value:.1f} {unit}"
        value /= 1024

    return f"{value:.1f} ПБ"


def safe_filename(value, max_length=100):
    value = re.sub(
        r'[<>:"/\\|?*\x00-\x1f]+',
        "_",
        str(value),
    )
    value = value.strip(" ._")
    return (value or "item")[:max_length]


def is_drive_root(path):
    path = normalize_path(path)
    drive, tail = os.path.splitdrive(path)
    return bool(drive) and tail in ("\\", "/")


def is_reparse_point(path):
    if not os.path.lexists(path):
        return False

    if os.path.islink(path):
        return True

    if os.name != "nt":
        return False

    try:
        attributes = ctypes.windll.kernel32.GetFileAttributesW(
            str(path)
        )

        if attributes == 0xFFFFFFFF:
            return False

        return bool(attributes & 0x0400)

    except Exception:
        return False


def is_inside(path, root, allow_equal=False):
    path = normalize_path(path)
    root = normalize_path(root)

    if not path or not root:
        return False

    try:
        common = os.path.commonpath([path, root])
    except ValueError:
        return False

    if os.path.normcase(path) == os.path.normcase(root):
        return allow_equal

    return os.path.normcase(common) == os.path.normcase(root)


def validate_delete_path(path, root):
    path = normalize_path(path)
    root = normalize_path(root)

    if not path or not root:
        return False, "Пустой путь"

    if is_drive_root(path):
        return False, "Корень диска защищён"

    if os.path.normcase(path) == os.path.normcase(root):
        return False, "Корень категории защищён"

    if not is_inside(path, root):
        return False, "Путь находится вне разрешённой папки"

    if is_reparse_point(path):
        return False, "Ссылки и reparse points запрещены"

    protected = {
        os.path.normcase(get_system_drive()),
        os.path.normcase(get_windows_dir()),
        os.path.normcase(get_user_profile()),
        os.path.normcase(
            normalize_path(os.environ.get("ProgramFiles", ""))
        ),
        os.path.normcase(
            normalize_path(os.environ.get("ProgramFiles(x86)", ""))
        ),
    }

    if os.path.normcase(path) in protected:
        return False, "Системный путь защищён"

    return True, ""


def make_writable(path):
    try:
        os.chmod(path, stat.S_IWRITE)
        return True
    except OSError:
        return False
