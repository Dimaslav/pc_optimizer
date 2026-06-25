import sys
import os
import shutil
import subprocess
import ctypes
import string
import logging
import re
import stat
import json
import hashlib
import webbrowser
import urllib.request
from logging.handlers import RotatingFileHandler
from datetime import datetime

try:
    import winreg
except ImportError:
    winreg = None

from PyQt5.QtWidgets import (
    QApplication, QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,
    QGridLayout, QLabel, QPushButton, QTabWidget, QListWidget,
    QListWidgetItem, QCheckBox, QMessageBox, QStatusBar,
    QAbstractItemView, QFrame, QTreeWidget, QTreeWidgetItem,
    QHeaderView, QProgressBar, QLineEdit, QSpinBox, QComboBox,
    QFileDialog, QTimeEdit, QTextEdit
)
from PyQt5.QtCore import Qt, QTimer, QThread, pyqtSignal, QTime
from PyQt5.QtGui import QPalette, QColor


# ------------------------------------------------------------
# Пути и версия
# ------------------------------------------------------------
APP_NAME = "PC Optimizer"
APP_VERSION = "2.0.0"

if getattr(sys, "frozen", False):
    BASE_DIR = os.path.dirname(sys.executable)
else:
    BASE_DIR = os.path.dirname(os.path.abspath(__file__))

USER_DATA_DIR = os.path.join(os.environ.get("LOCALAPPDATA", BASE_DIR), "PCOptimizer")
CONFIG_DIR = os.path.join(USER_DATA_DIR, "config")
BACKUP_DIR = os.path.join(CONFIG_DIR, "backups")
LOG_DIR = os.path.join(USER_DATA_DIR, "logs")
os.makedirs(CONFIG_DIR, exist_ok=True)
os.makedirs(BACKUP_DIR, exist_ok=True)
os.makedirs(LOG_DIR, exist_ok=True)

CONFIG_FILE = os.path.join(CONFIG_DIR, "settings.json")
LOG_FILE = os.path.join(LOG_DIR, "pc_optimizer.log")

UPDATE_INFO_URL = "https://example.com/pc-optimizer/version.json"
TASK_NAME = "PC Optimizer Cleanup"
STARTUP_RUN_PATH = r"Software\Microsoft\Windows\CurrentVersion\Run"
DEFAULT_SCHEDULED_ACTIONS = ["temp", "recycle", "thumbnails", "privacy", "games", "dns"]


# ------------------------------------------------------------
# Логирование
# ------------------------------------------------------------
handler = RotatingFileHandler(
    LOG_FILE,
    maxBytes=5 * 1024 * 1024,
    backupCount=3,
    encoding="utf-8"
)

logging.basicConfig(
    handlers=[handler],
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s"
)

logging.info("Программа запущена")


# ------------------------------------------------------------
# Вспомогательные функции
# ------------------------------------------------------------
def new_stats():
    return {
        "deleted": 0,
        "locked": 0,
        "access_denied": 0,
        "other_errors": 0,
        "bytes_freed": 0
    }


def merge_stats(total, stats):
    for key in total:
        total[key] += int(stats.get(key, 0) or 0)
    return total


def normalize_paths(paths):
    result = []
    seen = set()
    for p in paths:
        if not p:
            continue
        p = os.path.expandvars(os.path.expanduser(str(p).strip().strip('"')))
        if not p:
            continue
        key = os.path.normcase(os.path.normpath(p))
        if key not in seen:
            seen.add(key)
            result.append(p)
    return result


def parse_path_list(text):
    parts = re.split(r"[;\n,]+", text or "")
    return normalize_paths(parts)


def get_windows_dir():
    return os.environ.get("WINDIR", r"C:\Windows")


def get_user_profile_dir():
    return os.environ.get("USERPROFILE", os.path.expanduser("~"))


def get_system_drive_root():
    drive = os.environ.get("SystemDrive", "C:")
    return drive + "\\"


def get_desktop_dir():
    desktop = os.path.join(get_user_profile_dir(), "Desktop")
    if os.path.isdir(desktop):
        return desktop
    return get_user_profile_dir()


def format_size(size):
    if size is None:
        return ""
    if size == 0:
        return "0 Б"
    for unit in ["Б", "КБ", "МБ", "ГБ", "ТБ"]:
        if size < 1024:
            return f"{size:.1f} {unit}"
        size /= 1024
    return f"{size:.1f} ПБ"


def sanitize_filename(name, max_len=80):
    safe = re.sub(r'[<>:"/\\|?*\x00-\x1f]+', "_", str(name))
    safe = safe.strip(" ._")
    if not safe:
        safe = "startup_entry"
    return safe[:max_len]


def count_error(stats, exc):
    if isinstance(exc, FileNotFoundError):
        return
    if isinstance(exc, PermissionError):
        stats["access_denied"] += 1
        return

    if isinstance(exc, OSError):
        winerror = getattr(exc, "winerror", None)
        errno = getattr(exc, "errno", None)
        if winerror == 32:
            stats["locked"] += 1
        elif winerror == 5 or errno == 13:
            stats["access_denied"] += 1
        else:
            stats["other_errors"] += 1
        return

    stats["other_errors"] += 1


def _onerror_rmtree(func, path, exc_info):
    try:
        os.chmod(path, stat.S_IWRITE)
        func(path)
    except Exception:
        raise


def get_path_size(path):
    total = 0
    if not path or not os.path.exists(path):
        return 0

    try:
        if os.path.islink(path):
            return 0
        if os.path.isfile(path):
            try:
                return os.path.getsize(path)
            except OSError:
                return 0

        for root, dirs, files in os.walk(path):
            for f in files:
                fp = os.path.join(root, f)
                try:
                    total += os.path.getsize(fp)
                except OSError:
                    pass
    except OSError:
        pass
    return total


def get_folder_size(paths):
    total = 0
    for path in normalize_paths(paths):
        total += get_path_size(path)
    return total


def delete_file(path, stats):
    try:
        try:
            size = 0 if os.path.islink(path) else os.path.getsize(path)
        except OSError:
            size = 0

        os.remove(path)
        stats["deleted"] += 1
        stats["bytes_freed"] += size
        return True
    except Exception as e:
        try:
            os.chmod(path, stat.S_IWRITE)
            os.remove(path)
            stats["deleted"] += 1
            stats["bytes_freed"] += size
            return True
        except Exception as e2:
            count_error(stats, e2 if isinstance(e2, Exception) else e)
            return False


def delete_dir(path, stats):
    try:
        if os.path.islink(path):
            os.unlink(path)
            stats["deleted"] += 1
            return True

        size = get_path_size(path)
        shutil.rmtree(path, onerror=_onerror_rmtree)
        stats["deleted"] += 1
        stats["bytes_freed"] += size
        return True
    except Exception as e:
        count_error(stats, e)
        return False


def clean_tree_contents(folder, stats):
    if not folder or not os.path.exists(folder):
        return

    try:
        for root, dirs, files in os.walk(folder, topdown=False):
            for f in files:
                delete_file(os.path.join(root, f), stats)
            for d in dirs:
                delete_dir(os.path.join(root, d), stats)
    except Exception as e:
        count_error(stats, e)


def is_thumbnail_cache_file(filename):
    name = filename.lower()
    return (
        (name.startswith("thumbcache") or name.startswith("iconcache"))
        and name.endswith(".db")
    )


def get_thumbnail_cache_size():
    path = os.path.join(
        get_user_profile_dir(),
        "AppData", "Local", "Microsoft", "Windows", "Explorer"
    )

    total = 0
    if not os.path.exists(path):
        return 0

    try:
        for root, dirs, files in os.walk(path):
            for f in files:
                if is_thumbnail_cache_file(f):
                    fp = os.path.join(root, f)
                    try:
                        total += os.path.getsize(fp)
                    except OSError:
                        pass
    except OSError:
        pass

    return total


class SHQUERYRBINFO(ctypes.Structure):
    _fields_ = [
        ("cbSize", ctypes.c_ulong),
        ("i64Size", ctypes.c_longlong),
        ("i64NumItems", ctypes.c_longlong),
    ]


SHERB_NOCONFIRMATION = 0x00000001
SHERB_NOPROGRESSUI = 0x00000002
SHERB_NOSOUND = 0x00000004


def get_drive_letters():
    drives = []
    try:
        bitmask = ctypes.windll.kernel32.GetLogicalDrives()
        for letter in string.ascii_uppercase:
            if bitmask & 1:
                drive = f"{letter}:\\"
                if os.path.exists(drive):
                    drives.append(drive)
            bitmask >>= 1
    except Exception:
        logging.exception("Ошибка при получении списка дисков")
    return drives


def get_recycle_size_all_drives():
    total = 0
    try:
        for drive in get_drive_letters():
            info = SHQUERYRBINFO()
            info.cbSize = ctypes.sizeof(SHQUERYRBINFO)
            result = ctypes.windll.shell32.SHQueryRecycleBinW(
                ctypes.c_wchar_p(drive),
                ctypes.byref(info)
            )
            if result == 0:
                total += int(info.i64Size)
    except Exception:
        logging.exception("Ошибка при получении размера корзины")
    return total


def version_tuple(v):
    parts = re.findall(r"\d+", str(v))
    return tuple(int(x) for x in parts[:3]) if parts else (0, 0, 0)


def is_newer_version(remote, local=APP_VERSION):
    return version_tuple(remote) > version_tuple(local)


def load_settings():
    default = {
        "theme": "dark",
        "big_files_min_mb": 100,
        "duplicate_min_mb": 10,
        "auto_restore_point": True,
        "check_updates_on_start": True,
        "cleanup_schedule": "WEEKLY",
        "cleanup_time": "09:00",
        "scheduled_actions": DEFAULT_SCHEDULED_ACTIONS[:],
        "last_cleanup": "",
        "last_freed": 0
    }

    if not os.path.exists(CONFIG_FILE):
        return default

    try:
        with open(CONFIG_FILE, "r", encoding="utf-8") as f:
            data = json.load(f)
        if isinstance(data, dict):
            default.update(data)
    except Exception:
        logging.exception("Не удалось загрузить настройки")
    return default


def save_settings(settings):
    try:
        os.makedirs(CONFIG_DIR, exist_ok=True)
        with open(CONFIG_FILE, "w", encoding="utf-8") as f:
            json.dump(settings, f, ensure_ascii=False, indent=2)
        return True
    except Exception:
        logging.exception("Не удалось сохранить настройки")
        return False


def backup_settings():
    try:
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        target = os.path.join(BACKUP_DIR, f"backup_{timestamp}")
        os.makedirs(target, exist_ok=True)

        if os.path.exists(CONFIG_FILE):
            shutil.copy2(CONFIG_FILE, os.path.join(target, "settings.json"))

        if os.path.isdir(LOG_DIR):
            logs_target = os.path.join(target, "logs")
            shutil.copytree(LOG_DIR, logs_target, dirs_exist_ok=True)

        if winreg is not None:
            try:
                subprocess.run(
                    [
                        "reg", "export",
                        r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run",
                        os.path.join(target, "startup_hkcu.reg"),
                        "/y"
                    ],
                    capture_output=True,
                    shell=False,
                    check=False
                )
            except Exception:
                pass

            try:
                subprocess.run(
                    [
                        "reg", "export",
                        r"HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Run",
                        os.path.join(target, "startup_hklm.reg"),
                        "/y"
                    ],
                    capture_output=True,
                    shell=False,
                    check=False
                )
            except Exception:
                pass

        return target
    except Exception:
        logging.exception("Ошибка резервного копирования настроек")
        return None


def create_restore_point(description="PC Optimizer restore point"):
    try:
        safe_desc = description.replace("'", "''")
        cmd = f"Checkpoint-Computer -Description '{safe_desc}' -RestorePointType 'MODIFY_SETTINGS'"
        subprocess.run(
            ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", cmd],
            capture_output=True,
            shell=False,
            check=True
        )
        return True
    except Exception:
        logging.exception("Не удалось создать точку восстановления")
        return False


# ------------------------------------------------------------
# Автозагрузка
# ------------------------------------------------------------
def startup_display_root(root_kind, view_flag=0):
    if root_kind == "HKCU":
        return "HKCU"
    if root_kind == "HKLM":
        wow32 = getattr(winreg, "KEY_WOW64_32KEY", 0) if winreg else 0
        if view_flag == wow32:
            return "HKLM (32-bit)"
        return "HKLM (64-bit)"
    return root_kind


def startup_reg_export_path(root_kind, view_flag=0):
    if root_kind == "HKCU":
        return r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run"

    wow32 = getattr(winreg, "KEY_WOW64_32KEY", 0) if winreg else 0
    if view_flag == wow32:
        return r"HKEY_LOCAL_MACHINE\Software\Wow6432Node\Microsoft\Windows\CurrentVersion\Run"
    return r"HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Run"


def startup_reg_open_root(root_kind):
    if root_kind == "HKCU":
        return winreg.HKEY_CURRENT_USER
    return winreg.HKEY_LOCAL_MACHINE


def get_startup_folders():
    program_data = os.environ.get("ProgramData", r"C:\ProgramData")
    return normalize_paths([
        os.path.join(
            get_user_profile_dir(),
            "AppData", "Roaming", "Microsoft", "Windows", "Start Menu", "Programs", "Startup"
        ),
        os.path.join(
            program_data,
            "Microsoft", "Windows", "Start Menu", "Programs", "Startup"
        )
    ])


def enum_startup_registry_items(root_kind, view_flag=0):
    items = []
    if winreg is None:
        return items

    root = startup_reg_open_root(root_kind)
    access = winreg.KEY_READ | view_flag
    try:
        with winreg.OpenKey(root, STARTUP_RUN_PATH, 0, access) as key:
            i = 0
            while True:
                try:
                    name, value, reg_type = winreg.EnumValue(key, i)
                    items.append({
                        "kind": "registry",
                        "root_kind": root_kind,
                        "view_flag": view_flag,
                        "name": name,
                        "value": value,
                        "reg_type": reg_type,
                        "display_type": "Registry",
                        "display_name": name,
                        "display_source": startup_reg_export_path(root_kind, view_flag),
                        "display_value": str(value),
                        "tooltip": f"{startup_reg_export_path(root_kind, view_flag)}\n{name}\n{value}"
                    })
                    i += 1
                except OSError:
                    break
    except Exception:
        logging.exception("Ошибка чтения автозагрузки из реестра")
    return items


def enum_startup_folder_items():
    items = []
    for startup_dir in get_startup_folders():
        if not os.path.isdir(startup_dir):
            continue
        try:
            for entry in os.scandir(startup_dir):
                try:
                    if entry.is_file(follow_symlinks=False) or entry.is_symlink():
                        items.append({
                            "kind": "startup_file",
                            "path": entry.path,
                            "display_type": "Startup file",
                            "display_name": entry.name,
                            "display_source": startup_dir,
                            "display_value": entry.path,
                            "tooltip": entry.path
                        })
                except Exception:
                    logging.exception("Ошибка чтения элемента папки автозагрузки")
        except Exception:
            logging.exception("Ошибка чтения папки автозагрузки")
    return items


def collect_startup_items():
    items = []
    if winreg is not None:
        items.extend(enum_startup_registry_items("HKCU", 0))

        wow64_64 = getattr(winreg, "KEY_WOW64_64KEY", 0)
        wow64_32 = getattr(winreg, "KEY_WOW64_32KEY", 0)

        if sys.maxsize > 2**32:
            views = []
            if wow64_64:
                views.append(wow64_64)
            if wow64_32:
                views.append(wow64_32)
            if not views:
                views = [0]
        else:
            views = [0]

        for view in views:
            items.extend(enum_startup_registry_items("HKLM", view))

    items.extend(enum_startup_folder_items())

    seen = set()
    unique = []
    for item in items:
        key = (
            item.get("kind"),
            item.get("root_kind"),
            item.get("view_flag"),
            item.get("name"),
            repr(item.get("value")),
            item.get("path")
        )
        if key not in seen:
            seen.add(key)
            unique.append(item)
    return unique


def count_run_values(root_kind, view_flag=0):
    count = 0
    if winreg is None:
        return 0

    root = startup_reg_open_root(root_kind)
    access = winreg.KEY_READ | view_flag
    try:
        with winreg.OpenKey(root, STARTUP_RUN_PATH, 0, access) as key:
            i = 0
            while True:
                try:
                    winreg.EnumValue(key, i)
                    count += 1
                    i += 1
                except OSError:
                    break
    except Exception:
        pass
    return count


def get_startup_count():
    count = 0
    if winreg is not None:
        count += count_run_values("HKCU", 0)

        wow64_64 = getattr(winreg, "KEY_WOW64_64KEY", 0)
        wow64_32 = getattr(winreg, "KEY_WOW64_32KEY", 0)

        if sys.maxsize > 2**32:
            views = []
            if wow64_64:
                views.append(wow64_64)
            if wow64_32:
                views.append(wow64_32)
            if not views:
                views = [0]
        else:
            views = [0]

        for view in views:
            count += count_run_values("HKLM", view)

    count += len(enum_startup_folder_items())
    return count


def build_startup_backup_content(root_path, name, value, reg_type):
    reg_sz = getattr(winreg, "REG_SZ", 1) if winreg else 1
    reg_expand_sz = getattr(winreg, "REG_EXPAND_SZ", 2) if winreg else 2
    reg_binary = getattr(winreg, "REG_BINARY", 3) if winreg else 3
    reg_dword = getattr(winreg, "REG_DWORD", 4) if winreg else 4
    reg_multi_sz = getattr(winreg, "REG_MULTI_SZ", 7) if winreg else 7
    reg_qword = getattr(winreg, "REG_QWORD", 11) if winreg else 11

    def reg_escape_string(v):
        return str(v).replace("\\", "\\\\").replace('"', '\\"')

    def bytes_to_hex(data):
        return ",".join(f"{b:02x}" for b in data)

    timestamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    lines = [
        f"; Backup created by PC Optimizer on {timestamp}",
        "Windows Registry Editor Version 5.00",
        "",
        f"[{root_path}]"
    ]

    reg_name = f"\"{reg_escape_string(name)}\""

    if reg_type == reg_sz:
        lines.append(f'{reg_name}="{reg_escape_string(value)}"')
    elif reg_type == reg_expand_sz:
        raw = str(value).encode("utf-16le") + b"\x00\x00"
        lines.append(f"{reg_name}=hex(2):{bytes_to_hex(raw)}")
    elif reg_type == reg_dword:
        try:
            dword = int(value) & 0xFFFFFFFF
        except Exception:
            dword = 0
        lines.append(f"{reg_name}=dword:{dword:08x}")
    elif reg_type == reg_qword:
        try:
            qword = int(value).to_bytes(8, byteorder="little", signed=False)
            lines.append(f"{reg_name}=hex(b):{bytes_to_hex(qword)}")
        except Exception:
            lines.append(f'{reg_name}="{reg_escape_string(value)}"')
    elif reg_type == reg_multi_sz:
        if isinstance(value, (list, tuple)):
            joined = "\x00".join(str(x) for x in value) + "\x00"
        else:
            joined = str(value)
        raw = joined.encode("utf-16le") + b"\x00\x00"
        lines.append(f"{reg_name}=hex(7):{bytes_to_hex(raw)}")
    elif reg_type == reg_binary:
        try:
            data = bytes(value) if not isinstance(value, (bytes, bytearray)) else bytes(value)
            lines.append(f"{reg_name}=hex:{bytes_to_hex(data)}")
        except Exception:
            lines.append(f'{reg_name}="{reg_escape_string(value)}"')
    else:
        lines.append(f'{reg_name}="{reg_escape_string(value)}"')

    return "\r\n".join(lines) + "\r\n"


# ------------------------------------------------------------
# Очистка
# ------------------------------------------------------------
def clean_temp():
    stats = new_stats()
    paths = normalize_paths([
        os.environ.get("TEMP", ""),
        os.environ.get("TMP", ""),
        os.path.join(get_windows_dir(), "Temp")
    ])

    for folder in paths:
        clean_tree_contents(folder, stats)

    logging.info(
        f"Temp: удалено={stats['deleted']}, освобождено={format_size(stats['bytes_freed'])}, "
        f"locked={stats['locked']}, access_denied={stats['access_denied']}, other_errors={stats['other_errors']}"
    )
    return stats


def clean_recycle_winapi(expected_size=0):
    stats = new_stats()
    try:
        flags = SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND
        result = ctypes.windll.shell32.SHEmptyRecycleBinW(0, None, flags)
        if result == 0:
            stats["deleted"] = 1
            if expected_size:
                stats["bytes_freed"] += int(expected_size)
        else:
            stats["other_errors"] += 1
    except Exception as e:
        count_error(stats, e)

    logging.info(
        f"Корзина: удалено={stats['deleted']}, освобождено={format_size(stats['bytes_freed'])}, "
        f"locked={stats['locked']}, access_denied={stats['access_denied']}, other_errors={stats['other_errors']}"
    )
    return stats


def stop_services(service_names):
    for svc in service_names:
        subprocess.run(["net", "stop", svc], capture_output=True, shell=False, check=False)


def start_services(service_names):
    for svc in reversed(service_names):
        subprocess.run(["net", "start", svc], capture_output=True, shell=False, check=False)


def clean_updates_safe():
    stats = new_stats()
    path = os.path.join(get_windows_dir(), "SoftwareDistribution", "Download")

    if os.path.exists(path):
        services = ["wuauserv", "bits", "cryptsvc", "dosvc", "msiserver"]
        try:
            stop_services(services)
            clean_tree_contents(path, stats)
        except Exception as e:
            count_error(stats, e)
        finally:
            start_services(services)

    logging.info(
        f"Обновления: удалено={stats['deleted']}, освобождено={format_size(stats['bytes_freed'])}, "
        f"locked={stats['locked']}, access_denied={stats['access_denied']}, other_errors={stats['other_errors']}"
    )
    return stats


def clean_windows_old():
    stats = new_stats()
    path = os.path.join(get_system_drive_root(), "Windows.old")

    if os.path.exists(path):
        size = get_path_size(path)
        deleted = False
        try:
            shutil.rmtree(path, onerror=_onerror_rmtree)
            deleted = True
        except Exception as e:
            count_error(stats, e)
            try:
                subprocess.run(
                    ["cmd", "/c", f'rd /s /q "{path}"'],
                    capture_output=True,
                    shell=False,
                    check=True
                )
                deleted = True
            except Exception as e2:
                count_error(stats, e2)

        if deleted:
            stats["deleted"] = 1
            stats["bytes_freed"] += size

    logging.info(
        f"Windows.old: удалено={stats['deleted']}, освобождено={format_size(stats['bytes_freed'])}, "
        f"locked={stats['locked']}, access_denied={stats['access_denied']}, other_errors={stats['other_errors']}"
    )
    return stats


def clean_thumbnails():
    stats = new_stats()
    path = os.path.join(
        get_user_profile_dir(),
        "AppData", "Local", "Microsoft", "Windows", "Explorer"
    )

    if os.path.exists(path):
        try:
            for root, dirs, files in os.walk(path, topdown=False):
                for f in files:
                    if is_thumbnail_cache_file(f):
                        delete_file(os.path.join(root, f), stats)
        except Exception as e:
            count_error(stats, e)

    logging.info(
        f"Миниатюры: удалено={stats['deleted']}, освобождено={format_size(stats['bytes_freed'])}, "
        f"locked={stats['locked']}, access_denied={stats['access_denied']}, other_errors={stats['other_errors']}"
    )
    return stats


def clean_logs():
    stats = new_stats()
    paths = normalize_paths([
        os.path.join(get_windows_dir(), "Logs")
    ])

    for path in paths:
        clean_tree_contents(path, stats)

    logging.info(
        f"Логи: удалено={stats['deleted']}, освобождено={format_size(stats['bytes_freed'])}, "
        f"locked={stats['locked']}, access_denied={stats['access_denied']}, other_errors={stats['other_errors']}"
    )
    return stats


def clean_dns():
    stats = new_stats()
    try:
        subprocess.run(["ipconfig", "/flushdns"], capture_output=True, shell=False, check=True)
        stats["deleted"] = 1
    except Exception as e:
        count_error(stats, e)

    logging.info(
        f"DNS: удалено={stats['deleted']}, освобождено={format_size(stats['bytes_freed'])}, "
        f"locked={stats['locked']}, access_denied={stats['access_denied']}, other_errors={stats['other_errors']}"
    )
    return stats


def clean_recent():
    stats = new_stats()
    paths = normalize_paths([
        os.path.join(get_user_profile_dir(), "Recent"),
        os.path.join(get_user_profile_dir(), "AppData", "Roaming", "Microsoft", "Windows", "Recent")
    ])

    for path in paths:
        clean_tree_contents(path, stats)

    logging.info(
        f"Недавние: удалено={stats['deleted']}, освобождено={format_size(stats['bytes_freed'])}, "
        f"locked={stats['locked']}, access_denied={stats['access_denied']}, other_errors={stats['other_errors']}"
    )
    return stats


def get_browser_cache_paths():
    u = get_user_profile_dir()
    paths = []

    chrome_root = os.path.join(u, "AppData", "Local", "Google", "Chrome", "User Data", "Default")
    edge_root = os.path.join(u, "AppData", "Local", "Microsoft", "Edge", "User Data", "Default")

    for root in [chrome_root, edge_root]:
        paths.extend([
            os.path.join(root, "Cache"),
            os.path.join(root, "Code Cache"),
            os.path.join(root, "GPUCache")
        ])

    firefox_root = os.path.join(u, "AppData", "Local", "Mozilla", "Firefox", "Profiles")
    if os.path.isdir(firefox_root):
        try:
            for prof in os.scandir(firefox_root):
                if prof.is_dir():
                    paths.extend([
                        os.path.join(prof.path, "cache2"),
                        os.path.join(prof.path, "startupCache")
                    ])
        except Exception:
            pass

    paths.append(os.path.join(u, "AppData", "Local", "Microsoft", "Windows", "INetCache"))
    return normalize_paths(paths)


def clean_privacy():
    stats = new_stats()
    for path in get_browser_cache_paths():
        clean_tree_contents(path, stats)

    # Дополнительно чистим историю/Recent/Explorer cache
    clean_tree_contents(os.path.join(get_user_profile_dir(), "Recent"), stats)
    clean_tree_contents(
        os.path.join(get_user_profile_dir(), "AppData", "Roaming", "Microsoft", "Windows", "Recent"),
        stats
    )

    logging.info(
        f"Конфиденциальность: удалено={stats['deleted']}, освобождено={format_size(stats['bytes_freed'])}, "
        f"locked={stats['locked']}, access_denied={stats['access_denied']}, other_errors={stats['other_errors']}"
    )
    return stats


def get_game_cache_paths():
    u = get_user_profile_dir()
    paths = [
        os.path.join(u, "AppData", "Local", "D3DSCache"),
        os.path.join(u, "AppData", "Local", "NVIDIA", "DXCache"),
        os.path.join(u, "AppData", "Local", "NVIDIA", "GLCache"),
        os.path.join(u, "AppData", "Local", "NVIDIA Corporation", "NV_Cache"),
        os.path.join(u, "AppData", "Roaming", "NVIDIA", "ComputeCache"),
        os.path.join(u, "AppData", "Local", "Discord", "Cache"),
        os.path.join(u, "AppData", "Local", "Discord", "Code Cache"),
        os.path.join(u, "AppData", "Local", "Discord", "GPUCache"),
        os.path.join(u, "AppData", "Local", "EpicGamesLauncher", "Saved", "webcache"),
        os.path.join(u, "AppData", "Local", "EpicGamesLauncher", "Saved", "webcache_4147"),
        os.path.join(u, "AppData", "Local", "Battle.net", "Cache"),
        os.path.join(u, "AppData", "Local", "Battle.net", "BrowserCache"),
    ]

    pf_x86 = os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")
    pf = os.environ.get("ProgramFiles", r"C:\Program Files")

    steam_roots = [
        os.path.join(pf_x86, "Steam"),
        os.path.join(pf, "Steam"),
    ]
    for steam in steam_roots:
        paths.extend([
            os.path.join(steam, "appcache"),
            os.path.join(steam, "depotcache"),
            os.path.join(steam, "htmlcache"),
            os.path.join(steam, "shadercache"),
        ])

    return normalize_paths(paths)


def clean_game_caches():
    stats = new_stats()
    for path in get_game_cache_paths():
        clean_tree_contents(path, stats)

    logging.info(
        f"Игровые кэши: удалено={stats['deleted']}, освобождено={format_size(stats['bytes_freed'])}, "
        f"locked={stats['locked']}, access_denied={stats['access_denied']}, other_errors={stats['other_errors']}"
    )
    return stats


# ------------------------------------------------------------
# Сканирование / аналитика
# ------------------------------------------------------------
def iter_files(root_paths):
    for root in normalize_paths(root_paths):
        if not os.path.isdir(root):
            continue
        try:
            for base, dirs, files in os.walk(root):
                for name in files:
                    yield os.path.join(base, name)
        except Exception:
            pass


def file_sha256(path, chunk_size=1024 * 1024):
    h = hashlib.sha256()
    try:
        with open(path, "rb") as f:
            while True:
                chunk = f.read(chunk_size)
                if not chunk:
                    break
                h.update(chunk)
        return h.hexdigest()
    except Exception:
        return None


def scan_big_files(root_paths, min_size_mb=100, limit=200):
    min_size = int(min_size_mb) * 1024 * 1024
    results = []

    for file_path in iter_files(root_paths):
        try:
            size = os.path.getsize(file_path)
            if size >= min_size:
                results.append({
                    "path": file_path,
                    "size": size,
                    "size_str": format_size(size)
                })
        except Exception:
            pass

    results.sort(key=lambda x: x["size"], reverse=True)
    return results[:limit]


def scan_duplicates(root_paths, min_size_mb=10, limit_groups=200):
    min_size = int(min_size_mb) * 1024 * 1024
    size_buckets = {}

    for file_path in iter_files(root_paths):
        try:
            size = os.path.getsize(file_path)
            if size < min_size:
                continue
            size_buckets.setdefault(size, []).append(file_path)
        except Exception:
            pass

    groups = []
    for size, files in size_buckets.items():
        if len(files) < 2:
            continue

        hash_buckets = {}
        for p in files:
            h = file_sha256(p)
            if not h:
                continue
            hash_buckets.setdefault(h, []).append(p)

        for h, dupes in hash_buckets.items():
            if len(dupes) >= 2:
                groups.append({
                    "size": size,
                    "size_str": format_size(size),
                    "hash": h,
                    "count": len(dupes),
                    "files": dupes
                })

    groups.sort(key=lambda g: g["size"], reverse=True)
    return groups[:limit_groups]


def scan_disk_map(root_paths, limit=200):
    items = []
    for root in normalize_paths(root_paths):
        if not os.path.isdir(root):
            continue
        try:
            for entry in os.scandir(root):
                try:
                    size = get_path_size(entry.path)
                    items.append({
                        "name": entry.name,
                        "path": entry.path,
                        "size": size,
                        "size_str": format_size(size)
                    })
                except Exception:
                    pass
        except Exception:
            pass

    total = sum(x["size"] for x in items) or 1
    for x in items:
        x["percent"] = x["size"] * 100.0 / total

    items.sort(key=lambda x: x["size"], reverse=True)
    return items[:limit]


def list_processes():
    try:
        import psutil
    except Exception:
        return []

    rows = []
    try:
        for p in psutil.process_iter(["pid", "name", "username", "memory_info", "cpu_percent"]):
            try:
                info = p.info
                mem = info.get("memory_info")
                rss = mem.rss if mem else 0
                rows.append({
                    "pid": info.get("pid"),
                    "name": info.get("name") or "",
                    "user": info.get("username") or "",
                    "cpu": float(info.get("cpu_percent") or 0),
                    "memory": rss,
                    "memory_str": format_size(rss)
                })
            except Exception:
                pass
    except Exception:
        logging.exception("Ошибка получения списка процессов")

    rows.sort(key=lambda x: (x["memory"], x["cpu"]), reverse=True)
    return rows


def terminate_process(pid):
    try:
        import psutil
        p = psutil.Process(int(pid))
        p.terminate()
        try:
            p.wait(timeout=3)
        except Exception:
            p.kill()
        return True
    except Exception:
        logging.exception(f"Не удалось завершить процесс {pid}")
        return False


def list_services():
    try:
        import psutil
    except Exception:
        return []

    rows = []
    try:
        for s in psutil.win_service_iter():
            try:
                info = s.as_dict()
                rows.append({
                    "name": info.get("name", ""),
                    "display_name": info.get("display_name", ""),
                    "status": info.get("status", ""),
                    "start_type": info.get("start_type", "")
                })
            except Exception:
                pass
    except Exception:
        logging.exception("Ошибка чтения служб")
    return rows


def control_service(service_name, action):
    try:
        subprocess.run(["sc", action, service_name], capture_output=True, shell=False, check=False)
        return True
    except Exception:
        logging.exception(f"Не удалось {action} службу {service_name}")
        return False


def check_program_update():
    try:
        with urllib.request.urlopen(UPDATE_INFO_URL, timeout=5) as resp:
            data = json.loads(resp.read().decode("utf-8"))

        remote_version = data.get("version", "0.0.0")
        notes = data.get("notes", "")
        url = data.get("url", "")

        return {
            "available": is_newer_version(remote_version),
            "remote_version": remote_version,
            "notes": notes,
            "url": url
        }
    except Exception:
        logging.exception("Ошибка проверки обновлений")
        return {
            "available": False,
            "remote_version": None,
            "notes": "",
            "url": ""
        }


def system_health_snapshot():
    startup_count = get_startup_count()
    score = 100
    warnings = []

    cpu_percent = None
    mem_percent = None
    mem_used = None
    mem_total = None
    mem_available = None
    disk_used = None
    disk_total = None
    disk_free = None

    try:
        import psutil

        cpu_percent = float(psutil.cpu_percent(interval=0.1))
        mem = psutil.virtual_memory()
        mem_percent = float(mem.percent)
        mem_used = int(mem.used)
        mem_total = int(mem.total)
        mem_available = int(mem.available)

        disk = psutil.disk_usage(get_system_drive_root())
        disk_used = int(disk.used)
        disk_total = int(disk.total)
        disk_free = int(disk.free)

        free_pct = (disk_free / disk_total) * 100 if disk_total else 0

        if free_pct < 10:
            score -= 30
            warnings.append("На системном диске очень мало свободного места")
        elif free_pct < 20:
            score -= 15
            warnings.append("На системном диске мало свободного места")

        if mem_percent > 90:
            score -= 20
            warnings.append("Высокая загрузка ОЗУ")
        elif mem_percent > 75:
            score -= 10
            warnings.append("ОЗУ используется активно")

        if cpu_percent > 85:
            score -= 10
            warnings.append("Высокая нагрузка CPU")

    except Exception:
        warnings.append("psutil не установлен или недоступен")
        score = 50

    if startup_count > 20:
        score -= 15
        warnings.append(f"Много элементов автозагрузки: {startup_count}")
    elif startup_count > 10:
        score -= 8
        warnings.append(f"Автозагрузка содержит {startup_count} элементов")

    score = max(0, min(100, score))

    if score >= 85:
        state = "Отлично"
    elif score >= 65:
        state = "Нормально"
    elif score >= 40:
        state = "Есть проблемы"
    else:
        state = "Требует внимания"

    return {
        "score": score,
        "state": state,
        "warnings": warnings,
        "startup_count": startup_count,
        "cpu_percent": cpu_percent,
        "mem_percent": mem_percent,
        "mem_used": mem_used,
        "mem_total": mem_total,
        "mem_available": mem_available,
        "disk_used": disk_used,
        "disk_total": disk_total,
        "disk_free": disk_free
    }


def build_self_command(args=None):
    args = args or []
    if getattr(sys, "frozen", False):
        return subprocess.list2cmdline([sys.executable] + args)
    script = os.path.abspath(sys.argv[0])
    return subprocess.list2cmdline([sys.executable, script] + args)


def create_cleanup_task(schedule="WEEKLY", time_str="09:00"):
    try:
        cmd = build_self_command(["--auto-clean"])
        result = subprocess.run(
            [
                "schtasks", "/Create",
                "/TN", TASK_NAME,
                "/TR", cmd,
                "/SC", schedule,
                "/ST", time_str,
                "/RL", "HIGHEST",
                "/F"
            ],
            capture_output=True,
            shell=False,
            check=False,
            text=True
        )
        if result.returncode == 0:
            return True
        logging.error(f"Не удалось создать задачу планировщика: {result.stderr}")
        return False
    except Exception:
        logging.exception("Не удалось создать задачу планировщика")
        return False


def delete_cleanup_task():
    try:
        result = subprocess.run(
            ["schtasks", "/Delete", "/TN", TASK_NAME, "/F"],
            capture_output=True,
            shell=False,
            check=False,
            text=True
        )
        if result.returncode == 0:
            return True
        logging.error(f"Не удалось удалить задачу планировщика: {result.stderr}")
        return False
    except Exception:
        logging.exception("Не удалось удалить задачу планировщика")
        return False


def run_headless_auto_cleanup():
    settings = load_settings()
    actions = settings.get("scheduled_actions") or DEFAULT_SCHEDULED_ACTIONS[:]
    actions = list(dict.fromkeys(actions))

    dangerous = any(x in {"updates", "windows_old"} for x in actions)
    if dangerous and settings.get("auto_restore_point", True):
        create_restore_point("PC Optimizer scheduled cleanup")

    mapping = {
        "temp": clean_temp,
        "recycle": lambda: clean_recycle_winapi(get_recycle_size_all_drives()),
        "thumbnails": clean_thumbnails,
        "logs": clean_logs,
        "dns": clean_dns,
        "recent": clean_recent,
        "privacy": clean_privacy,
        "games": clean_game_caches,
        "updates": clean_updates_safe,
        "windows_old": clean_windows_old
    }

    total = new_stats()
    print(f"[{APP_NAME}] Auto-clean start")
    logging.info("Автоматическая очистка запущена")

    for key in actions:
        func = mapping.get(key)
        if not func:
            continue
        try:
            stats = func()
            merge_stats(total, stats)
        except Exception:
            logging.exception(f"Ошибка авто-очистки: {key}")
            total["other_errors"] += 1

    settings["last_cleanup"] = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    settings["last_freed"] = total["bytes_freed"]
    save_settings(settings)

    summary = (
        f"Auto-clean finished: deleted={total['deleted']}, "
        f"freed={format_size(total['bytes_freed'])}, locked={total['locked']}, "
        f"access_denied={total['access_denied']}, other_errors={total['other_errors']}"
    )
    print(summary)
    logging.info(summary)
    return 0


# ------------------------------------------------------------
# Потоки
# ------------------------------------------------------------
class AnalysisThread(QThread):
    progress = pyqtSignal(str)
    analysisFinished = pyqtSignal(list)

    def run(self):
        issues = []

        def add_issue(issue_id, category, description, size, dangerous, action):
            issues.append({
                "id": issue_id,
                "category": category,
                "description": description,
                "size": size,
                "size_str": format_size(size),
                "checked": True,
                "dangerous": dangerous,
                "action": action
            })

        try:
            logging.info("Начато сканирование системы")
            self.progress.emit("Сканирование системы...")

            self.progress.emit("Проверка временных файлов...")
            temp_size = get_folder_size([
                os.environ.get("TEMP", ""),
                os.environ.get("TMP", ""),
                os.path.join(get_windows_dir(), "Temp")
            ])
            if temp_size > 0:
                add_issue("temp", "Системный мусор", "Временные файлы Windows (Temp)", temp_size, False, clean_temp)

            self.progress.emit("Проверка корзины...")
            recycle_size = get_recycle_size_all_drives()
            if recycle_size > 0:
                add_issue(
                    "recycle",
                    "Системный мусор",
                    "Корзина (все диски)",
                    recycle_size,
                    False,
                    lambda expected_size=recycle_size: clean_recycle_winapi(expected_size)
                )

            self.progress.emit("Проверка файлов обновлений Windows...")
            updates_path = [os.path.join(get_windows_dir(), "SoftwareDistribution", "Download")]
            update_size = get_folder_size(updates_path)
            if update_size > 0:
                add_issue(
                    "updates",
                    "Обновления",
                    "Файлы обновлений Windows (SoftwareDistribution\\Download)",
                    update_size,
                    True,
                    clean_updates_safe
                )

            self.progress.emit("Проверка Windows.old...")
            old_size = get_folder_size([os.path.join(get_system_drive_root(), "Windows.old")])
            if old_size > 0:
                add_issue(
                    "windows_old",
                    "Системный мусор",
                    "Предыдущие версии Windows (Windows.old)",
                    old_size,
                    True,
                    clean_windows_old
                )

            self.progress.emit("Проверка кэша миниатюр...")
            thumb_size = get_thumbnail_cache_size()
            if thumb_size > 0:
                add_issue(
                    "thumbnails",
                    "Кэш",
                    "Кэш миниатюр (Thumbnails)",
                    thumb_size,
                    False,
                    clean_thumbnails
                )

            self.progress.emit("Проверка журналов системы...")
            logs_size = get_folder_size([os.path.join(get_windows_dir(), "Logs")])
            if logs_size > 0:
                add_issue(
                    "logs",
                    "Журналы",
                    "Журналы Windows (Logs)",
                    logs_size,
                    False,
                    clean_logs
                )

            self.progress.emit("Проверка недавних документов...")
            recent_size = get_folder_size([
                os.path.join(get_user_profile_dir(), "Recent"),
                os.path.join(get_user_profile_dir(), "AppData", "Roaming", "Microsoft", "Windows", "Recent")
            ])
            if recent_size > 0:
                add_issue(
                    "recent",
                    "История",
                    "Недавние документы",
                    recent_size,
                    False,
                    clean_recent
                )

            self.progress.emit("Проверка приватных кэшей...")
            privacy_size = get_folder_size(get_browser_cache_paths())
            if privacy_size > 0:
                add_issue(
                    "privacy",
                    "Конфиденциальность",
                    "Кэши браузеров и Internet cache",
                    privacy_size,
                    False,
                    clean_privacy
                )

            self.progress.emit("Проверка игровых кэшей...")
            games_size = get_folder_size(get_game_cache_paths())
            if games_size > 0:
                add_issue(
                    "games",
                    "Игры",
                    "Игровые кэши (Steam / Discord / Epic / NVIDIA)",
                    games_size,
                    False,
                    clean_game_caches
                )

            add_issue(
                "dns",
                "Кэш",
                "Кэш DNS (ipconfig /flushdns)",
                None,
                False,
                clean_dns
            )

            logging.info(f"Сканирование завершено. Найдено проблем: {len(issues)}")
            self.progress.emit("Сканирование завершено")
        except Exception:
            logging.exception("Ошибка во время анализа системы")
            self.progress.emit("Ошибка во время анализа системы")
        finally:
            self.analysisFinished.emit(issues)


class FixThread(QThread):
    progress = pyqtSignal(str)
    fixFinished = pyqtSignal(dict)

    def __init__(self, selected_actions):
        super().__init__()
        self.selected_actions = selected_actions

    def run(self):
        total = new_stats()
        try:
            total_actions = len(self.selected_actions)

            for i, (description, action) in enumerate(self.selected_actions):
                try:
                    self.progress.emit(f"Выполняется: {description} ({i + 1}/{total_actions})")
                    stats = action()
                    if not isinstance(stats, dict):
                        stats = new_stats()
                    merge_stats(total, stats)
                    self.progress.emit(
                        f"✅ Готово: {description} ({i + 1}/{total_actions}), "
                        f"удалено: {stats.get('deleted', 0)}, освобождено: {format_size(stats.get('bytes_freed', 0))}"
                    )
                except Exception as e:
                    total["other_errors"] += 1
                    logging.exception(f"Ошибка выполнения действия: {description}")
                    self.progress.emit(f"❌ Ошибка: {description} — {str(e)}")

            logging.info(
                f"Очистка завершена: удалено={total['deleted']}, освобождено={format_size(total['bytes_freed'])}, "
                f"locked={total['locked']}, access_denied={total['access_denied']}, other_errors={total['other_errors']}"
            )
        finally:
            self.fixFinished.emit(total)


class BigFilesThread(QThread):
    progress = pyqtSignal(str)
    finishedData = pyqtSignal(list)

    def __init__(self, roots, min_size_mb=100):
        super().__init__()
        self.roots = roots
        self.min_size_mb = min_size_mb

    def run(self):
        try:
            self.progress.emit("Идёт поиск больших файлов...")
            data = scan_big_files(self.roots, self.min_size_mb)
            self.finishedData.emit(data)
        except Exception:
            logging.exception("Ошибка поиска больших файлов")
            self.finishedData.emit([])


class DuplicatesThread(QThread):
    progress = pyqtSignal(str)
    finishedData = pyqtSignal(list)

    def __init__(self, roots, min_size_mb=10):
        super().__init__()
        self.roots = roots
        self.min_size_mb = min_size_mb

    def run(self):
        try:
            self.progress.emit("Идёт поиск дубликатов...")
            data = scan_duplicates(self.roots, self.min_size_mb)
            self.finishedData.emit(data)
        except Exception:
            logging.exception("Ошибка поиска дубликатов")
            self.finishedData.emit([])


class DiskMapThread(QThread):
    progress = pyqtSignal(str)
    finishedData = pyqtSignal(list)

    def __init__(self, roots):
        super().__init__()
        self.roots = roots

    def run(self):
        try:
            self.progress.emit("Построение карты диска...")
            data = scan_disk_map(self.roots)
            self.finishedData.emit(data)
        except Exception:
            logging.exception("Ошибка построения карты диска")
            self.finishedData.emit([])


class ProcessesThread(QThread):
    progress = pyqtSignal(str)
    finishedData = pyqtSignal(list)

    def run(self):
        try:
            self.progress.emit("Загрузка процессов...")
            self.finishedData.emit(list_processes())
        except Exception:
            logging.exception("Ошибка загрузки процессов")
            self.finishedData.emit([])


class ServicesThread(QThread):
    progress = pyqtSignal(str)
    finishedData = pyqtSignal(list)

    def run(self):
        try:
            self.progress.emit("Загрузка служб...")
            self.finishedData.emit(list_services())
        except Exception:
            logging.exception("Ошибка загрузки служб")
            self.finishedData.emit([])


class HealthThread(QThread):
    progress = pyqtSignal(str)
    finishedData = pyqtSignal(dict)

    def run(self):
        try:
            self.progress.emit("Оценка здоровья системы...")
            self.finishedData.emit(system_health_snapshot())
        except Exception:
            logging.exception("Ошибка расчёта здоровья системы")
            self.finishedData.emit({"score": 0, "state": "Ошибка", "warnings": []})


class UpdateCheckThread(QThread):
    progress = pyqtSignal(str)
    finishedData = pyqtSignal(dict)

    def run(self):
        try:
            self.progress.emit("Проверка обновлений...")
            self.finishedData.emit(check_program_update())
        except Exception:
            logging.exception("Ошибка проверки обновлений")
            self.finishedData.emit({"available": False})


# ------------------------------------------------------------
# Стиль
# ------------------------------------------------------------
FLUENT_QSS = """
QMainWindow {
    background: #0f1115;
}
QWidget {
    color: #e8e8e8;
    font-size: 13px;
}
QTabWidget::pane {
    border: none;
    background: #0f1115;
}
QTabBar::tab {
    background: #171a21;
    color: #bfc7d5;
    border: 1px solid #242a35;
    padding: 10px 18px;
    margin-right: 4px;
    border-top-left-radius: 10px;
    border-top-right-radius: 10px;
}
QTabBar::tab:selected {
    background: #1f6feb;
    color: white;
}
QTabBar::tab:hover {
    background: #202635;
}
QFrame#Card {
    background: #151922;
    border: 1px solid #242a35;
    border-radius: 14px;
}
QPushButton {
    background: #1f6feb;
    color: white;
    border: none;
    border-radius: 10px;
    padding: 10px 16px;
    font-weight: 600;
}
QPushButton:hover {
    background: #2b7cff;
}
QPushButton:pressed {
    background: #1858b8;
}
QPushButton:disabled {
    background: #404857;
    color: #8c93a1;
}
QLineEdit, QSpinBox, QComboBox, QTimeEdit {
    background: #11151d;
    border: 1px solid #2b3240;
    border-radius: 8px;
    padding: 8px;
    color: #e8e8e8;
}
QTextEdit {
    background: #11151d;
    border: 1px solid #2b3240;
    border-radius: 10px;
    color: #e8e8e8;
}
QListWidget, QTreeWidget {
    background: #11151d;
    border: 1px solid #2b3240;
    border-radius: 10px;
    color: #e8e8e8;
}
QHeaderView::section {
    background: #171b24;
    color: #d7dce7;
    padding: 6px;
    border: none;
}
QProgressBar {
    background: #11151d;
    border: 1px solid #2b3240;
    border-radius: 8px;
    text-align: center;
    height: 14px;
    color: #e8e8e8;
}
QProgressBar::chunk {
    background: #1f6feb;
    border-radius: 8px;
}
QCheckBox::indicator {
    width: 18px;
    height: 18px;
}
QCheckBox::indicator:unchecked {
    background: #11151d;
    border: 1px solid #4a5568;
    border-radius: 4px;
}
QCheckBox::indicator:checked {
    background: #1f6feb;
    border: 1px solid #2b7cff;
    border-radius: 4px;
}
"""


# ------------------------------------------------------------
# Главное окно
# ------------------------------------------------------------
class PCOptimizer(QMainWindow):
    def __init__(self):
        super().__init__()

        self.settings = load_settings()
        self.update_info = None
        self.analysis_thread = None
        self.fix_thread = None
        self.big_files_thread = None
        self.duplicates_thread = None
        self.disk_map_thread = None
        self.processes_thread = None
        self.services_thread = None
        self.health_thread = None
        self.update_thread = None

        self.status_bar = None
        self.issues = []

        self.use_psutil = False
        self.wmi = None

        self.btn_scan = None
        self.btn_fix = None
        self.progress_label = None
        self.fix_status = None
        self.issues_list = None
        self.startup_list = None
        self.progress_bar = None
        self.cleanup_progress = None

        self.init_ui()
        self.apply_styles()
        self.start_system_monitor()
        logging.info("Главное окно инициализировано")

        self.refresh_dashboard()

        self.dashboard_timer = QTimer(self)
        self.dashboard_timer.timeout.connect(self.refresh_dashboard)
        self.dashboard_timer.start(3000)

        self.tabs.currentChanged.connect(self.on_tab_changed)

        if self.settings.get("check_updates_on_start", True):
            QTimer.singleShot(2000, self.check_updates)

    def safe_status(self, msg: str):
        if hasattr(self, "status_bar") and self.status_bar is not None:
            self.status_bar.showMessage(msg)
        else:
            logging.info(msg)

    def make_metric_card(self, title, value="...", subtitle=""):
        card = QFrame()
        card.setObjectName("Card")
        layout = QVBoxLayout(card)
        layout.setContentsMargins(16, 16, 16, 16)
        layout.setSpacing(6)

        title_label = QLabel(title)
        title_label.setStyleSheet("color: #9aa4b2; font-size: 12px;")
        value_label = QLabel(value)
        value_label.setStyleSheet("color: white; font-size: 24px; font-weight: 800;")
        subtitle_label = QLabel(subtitle)
        subtitle_label.setStyleSheet("color: #c7cfdb; font-size: 12px;")

        layout.addWidget(title_label)
        layout.addWidget(value_label)
        if subtitle:
            layout.addWidget(subtitle_label)
        return card, value_label

    def any_worker_running(self):
        workers = [
            self.analysis_thread, self.fix_thread, self.big_files_thread,
            self.duplicates_thread, self.disk_map_thread, self.processes_thread,
            self.services_thread, self.health_thread, self.update_thread
        ]
        for t in workers:
            if t and t.isRunning():
                return True
        return False

    def collect_roots_from_edit(self, edit, fallback=None):
        roots = parse_path_list(edit.text())
        if not roots and fallback:
            roots = [fallback]
        return roots

    def open_item_path(self, item, column=0):
        try:
            path = item.data(0, Qt.UserRole)
            if not path or not os.path.exists(path):
                return
            target = path if os.path.isdir(path) else os.path.dirname(path)
            if target and os.path.exists(target):
                os.startfile(target)
        except Exception:
            pass

    def add_folder_to_edit(self, edit):
        folder = QFileDialog.getExistingDirectory(self, "Выберите папку", get_user_profile_dir())
        if folder:
            roots = parse_path_list(edit.text())
            if folder not in roots:
                roots.append(folder)
            edit.setText(";".join(roots))

    def collect_ui_settings(self):
        self.settings["theme"] = "dark"
        self.settings["auto_restore_point"] = self.chk_auto_restore_point.isChecked()
        self.settings["check_updates_on_start"] = self.chk_check_updates_on_start.isChecked()
        self.settings["big_files_min_mb"] = int(self.spin_big_files_min.value())
        self.settings["duplicate_min_mb"] = int(self.spin_duplicate_min.value())
        self.settings["cleanup_schedule"] = self.schedule_combo.currentData()
        self.settings["cleanup_time"] = self.schedule_time.time().toString("HH:mm")
        self.settings["scheduled_actions"] = [
            key for key, cb in self.schedule_checks.items() if cb.isChecked()
        ]
        return self.settings

    def save_ui_settings(self, silent=False):
        self.collect_ui_settings()
        ok = save_settings(self.settings)
        if ok:
            self.safe_status("Настройки сохранены")
            if not silent:
                QMessageBox.information(self, "Настройки", "Настройки успешно сохранены.")
            return True
        if not silent:
            QMessageBox.warning(self, "Ошибка", "Не удалось сохранить настройки.")
        return False

    def do_backup_settings(self):
        self.save_ui_settings(silent=True)
        target = backup_settings()
        if target:
            QMessageBox.information(self, "Резервная копия", f"Настройки сохранены в:\n{target}")
            self.safe_status("Резервная копия создана")
        else:
            QMessageBox.warning(self, "Ошибка", "Не удалось создать резервную копию.")

    def create_restore_point_now(self):
        reply = QMessageBox.question(
            self,
            "Точка восстановления",
            "Создать точку восстановления Windows сейчас?",
            QMessageBox.Yes | QMessageBox.No
        )
        if reply != QMessageBox.Yes:
            return
        self.safe_status("Создание точки восстановления...")
        QApplication.processEvents()
        if create_restore_point("PC Optimizer manual restore point"):
            QMessageBox.information(self, "Успех", "Точка восстановления создана.")
            self.safe_status("Точка восстановления создана")
        else:
            QMessageBox.warning(self, "Ошибка", "Не удалось создать точку восстановления.")

    def apply_styles(self):
        self.setStyleSheet(FLUENT_QSS)

        palette = QPalette()
        palette.setColor(QPalette.Window, QColor(15, 17, 21))
        palette.setColor(QPalette.WindowText, QColor(232, 232, 232))
        palette.setColor(QPalette.Base, QColor(17, 21, 29))
        palette.setColor(QPalette.AlternateBase, QColor(23, 27, 36))
        palette.setColor(QPalette.Text, QColor(232, 232, 232))
        palette.setColor(QPalette.Button, QColor(31, 111, 235))
        palette.setColor(QPalette.ButtonText, QColor(255, 255, 255))
        palette.setColor(QPalette.Highlight, QColor(31, 111, 235))
        palette.setColor(QPalette.HighlightedText, QColor(255, 255, 255))
        self.setPalette(palette)

    def init_ui(self):
        self.setWindowTitle(f"{APP_NAME} — Безопасная очистка системы")
        self.setMinimumSize(1200, 780)

        self.tabs = QTabWidget()
        self.tabs.setDocumentMode(True)
        self.setCentralWidget(self.tabs)

        # Статус-бар создаём до вкладок
        self.status_bar = QStatusBar()
        self.status_bar.setStyleSheet("""
            QStatusBar {
                background: #101318;
                color: #cfd7e3;
                border-top: 1px solid #242a35;
            }
        """)
        self.setStatusBar(self.status_bar)
        self.status_bar.showMessage("Готово")

        self.init_dashboard_tab()
        self.init_cleanup_tab()
        self.init_big_files_tab()
        self.init_duplicates_tab()
        self.init_disk_map_tab()
        self.init_health_tab()
        self.init_processes_tab()
        self.init_services_tab()
        self.init_startup_tab()
        self.init_privacy_tab()
        self.init_games_tab()
        self.init_scheduler_tab()
        self.init_updates_tab()
        self.init_settings_tab()

    def init_dashboard_tab(self):
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(14)

        title = QLabel(APP_NAME)
        title.setStyleSheet("font-size: 30px; font-weight: 900; color: white;")
        subtitle = QLabel("Настоящий Dashboard для управления системой")
        subtitle.setStyleSheet("color: #aab3c2; font-size: 13px;")
        layout.addWidget(title)
        layout.addWidget(subtitle)

        self.card_health, self.lbl_dash_health = self.make_metric_card("Здоровье системы", "—", "Общий статус")
        self.card_disk, self.lbl_dash_disk = self.make_metric_card("Свободно на диске C:", "—", "Системный диск")
        self.card_ram, self.lbl_dash_ram = self.make_metric_card("ОЗУ", "—", "Использование памяти")
        self.card_cpu, self.lbl_dash_cpu = self.make_metric_card("CPU", "—", "Загрузка процессора")
        self.card_startup, self.lbl_dash_startup = self.make_metric_card("Автозагрузка", "—", "Элементов")
        self.card_update, self.lbl_dash_update = self.make_metric_card("Обновления", "—", "Статус версии")

        grid = QGridLayout()
        grid.setHorizontalSpacing(12)
        grid.setVerticalSpacing(12)
        grid.addWidget(self.card_health, 0, 0)
        grid.addWidget(self.card_disk, 0, 1)
        grid.addWidget(self.card_ram, 0, 2)
        grid.addWidget(self.card_cpu, 1, 0)
        grid.addWidget(self.card_startup, 1, 1)
        grid.addWidget(self.card_update, 1, 2)
        layout.addLayout(grid)

        self.dashboard_health_bar = QProgressBar()
        self.dashboard_health_bar.setRange(0, 100)
        self.dashboard_health_bar.setFormat("%p%")
        layout.addWidget(self.dashboard_health_bar)

        self.lbl_last_cleanup = QLabel("Последняя очистка: —")
        self.lbl_last_cleanup.setStyleSheet("color: #cfd7e3; font-size: 13px; padding: 4px;")
        layout.addWidget(self.lbl_last_cleanup)

        btn_row = QHBoxLayout()
        self.btn_dashboard_scan = QPushButton("Быстрый анализ")
        self.btn_dashboard_scan.clicked.connect(self.start_analysis)

        self.btn_dashboard_backup = QPushButton("Бэкап настроек")
        self.btn_dashboard_backup.clicked.connect(self.do_backup_settings)

        self.btn_dashboard_updates = QPushButton("Проверить обновления")
        self.btn_dashboard_updates.clicked.connect(self.check_updates)

        btn_row.addWidget(self.btn_dashboard_scan)
        btn_row.addWidget(self.btn_dashboard_backup)
        btn_row.addWidget(self.btn_dashboard_updates)
        btn_row.addStretch()
        layout.addLayout(btn_row)

        layout.addStretch()
        self.tabs.addTab(w, "Dashboard")

    def init_cleanup_tab(self):
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(12)

        title = QLabel("Анализ и очистка")
        title.setStyleSheet("font-size: 22px; font-weight: 800; color: white;")
        layout.addWidget(title)

        self.btn_scan = QPushButton("Выполнить анализ")
        self.btn_scan.clicked.connect(self.start_analysis)
        layout.addWidget(self.btn_scan)

        self.progress_label = QLabel("Нажмите «Выполнить анализ» для сканирования системы")
        self.progress_label.setStyleSheet("color: #aab3c2;")
        layout.addWidget(self.progress_label)

        self.cleanup_progress = QProgressBar()
        self.cleanup_progress.setRange(0, 1)
        self.cleanup_progress.setValue(0)
        self.cleanup_progress.setTextVisible(False)
        layout.addWidget(self.cleanup_progress)

        self.issues_list = QListWidget()
        self.issues_list.setSelectionMode(QAbstractItemView.NoSelection)
        layout.addWidget(self.issues_list)

        self.btn_fix = QPushButton("Очистить выбранное")
        self.btn_fix.setEnabled(False)
        self.btn_fix.clicked.connect(self.start_fix)
        layout.addWidget(self.btn_fix)

        self.chk_auto_restore_point = QCheckBox("Создавать точку восстановления перед опасными действиями")
        self.chk_auto_restore_point.setChecked(self.settings.get("auto_restore_point", True))
        layout.addWidget(self.chk_auto_restore_point)

        self.fix_status = QLabel("")
        self.fix_status.setStyleSheet("color: #aab3c2;")
        layout.addWidget(self.fix_status)

        self.tabs.addTab(w, "Очистка")

    def init_big_files_tab(self):
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(12)

        title = QLabel("Анализ больших файлов")
        title.setStyleSheet("font-size: 22px; font-weight: 800;")
        layout.addWidget(title)

        row = QHBoxLayout()
        self.edit_big_roots = QLineEdit(get_user_profile_dir())
        self.btn_big_browse = QPushButton("Добавить папку")
        self.btn_big_browse.clicked.connect(lambda: self.add_folder_to_edit(self.edit_big_roots))
        self.btn_big_scan = QPushButton("Сканировать")
        self.btn_big_scan.clicked.connect(self.start_big_files_scan)
        row.addWidget(QLabel("Папки:"))
        row.addWidget(self.edit_big_roots, 1)
        row.addWidget(self.btn_big_browse)
        row.addWidget(self.btn_big_scan)
        layout.addLayout(row)

        self.big_files_note = QLabel(
            f"Порог: {self.settings.get('big_files_min_mb', 100)} МБ (из настроек)"
        )
        self.big_files_note.setStyleSheet("color: #aab3c2;")
        layout.addWidget(self.big_files_note)

        self.big_files_list = QTreeWidget()
        self.big_files_list.setHeaderLabels(["Путь", "Размер"])
        self.big_files_list.header().setSectionResizeMode(0, QHeaderView.Stretch)
        self.big_files_list.header().setSectionResizeMode(1, QHeaderView.ResizeToContents)
        self.big_files_list.itemDoubleClicked.connect(self.open_item_path)
        layout.addWidget(self.big_files_list)

        self.tabs.addTab(w, "Большие файлы")

    def init_duplicates_tab(self):
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(12)

        title = QLabel("Поиск дубликатов")
        title.setStyleSheet("font-size: 22px; font-weight: 800;")
        layout.addWidget(title)

        row = QHBoxLayout()
        self.edit_dup_roots = QLineEdit(get_user_profile_dir())
        self.btn_dup_browse = QPushButton("Добавить папку")
        self.btn_dup_browse.clicked.connect(lambda: self.add_folder_to_edit(self.edit_dup_roots))
        self.btn_dup_scan = QPushButton("Искать дубликаты")
        self.btn_dup_scan.clicked.connect(self.start_duplicates_scan)
        row.addWidget(QLabel("Папки:"))
        row.addWidget(self.edit_dup_roots, 1)
        row.addWidget(self.btn_dup_browse)
        row.addWidget(self.btn_dup_scan)
        layout.addLayout(row)

        self.dup_note = QLabel(
            f"Минимальный размер: {self.settings.get('duplicate_min_mb', 10)} МБ (из настроек)"
        )
        self.dup_note.setStyleSheet("color: #aab3c2;")
        layout.addWidget(self.dup_note)

        self.dup_list = QTreeWidget()
        self.dup_list.setHeaderLabels(["Группа / Путь", "Размер"])
        self.dup_list.header().setSectionResizeMode(0, QHeaderView.Stretch)
        self.dup_list.header().setSectionResizeMode(1, QHeaderView.ResizeToContents)
        self.dup_list.itemDoubleClicked.connect(self.open_item_path)
        layout.addWidget(self.dup_list)

        self.tabs.addTab(w, "Дубликаты")

    def init_disk_map_tab(self):
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(12)

        title = QLabel("Карта заполнения диска")
        title.setStyleSheet("font-size: 22px; font-weight: 800;")
        layout.addWidget(title)

        row = QHBoxLayout()
        self.disk_drive_combo = QComboBox()
        self.populate_drive_combo()
        self.btn_disk_scan = QPushButton("Построить карту")
        self.btn_disk_scan.clicked.connect(self.start_disk_map_scan)
        row.addWidget(QLabel("Диск:"))
        row.addWidget(self.disk_drive_combo)
        row.addWidget(self.btn_disk_scan)
        row.addStretch()
        layout.addLayout(row)

        self.disk_map_list = QTreeWidget()
        self.disk_map_list.setHeaderLabels(["Папка", "Размер", "Доля"])
        self.disk_map_list.header().setSectionResizeMode(0, QHeaderView.Stretch)
        self.disk_map_list.header().setSectionResizeMode(1, QHeaderView.ResizeToContents)
        self.disk_map_list.header().setSectionResizeMode(2, QHeaderView.ResizeToContents)
        self.disk_map_list.itemDoubleClicked.connect(self.open_item_path)
        layout.addWidget(self.disk_map_list)

        self.tabs.addTab(w, "Карта диска")

    def init_health_tab(self):
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(12)

        title = QLabel("Центр здоровья системы")
        title.setStyleSheet("font-size: 22px; font-weight: 800;")
        layout.addWidget(title)

        self.health_score_label = QLabel("Здоровье: —")
        self.health_state_label = QLabel("Статус: —")
        self.health_score_label.setStyleSheet("font-size: 18px; font-weight: 800;")
        self.health_state_label.setStyleSheet("font-size: 14px; color: #aab3c2;")
        layout.addWidget(self.health_score_label)
        layout.addWidget(self.health_state_label)

        self.health_progress = QProgressBar()
        self.health_progress.setRange(0, 100)
        self.health_progress.setFormat("%p%")
        layout.addWidget(self.health_progress)

        info_row = QGridLayout()
        self.health_cpu_label = QLabel("CPU: —")
        self.health_ram_label = QLabel("RAM: —")
        self.health_disk_label = QLabel("Disk: —")
        self.health_startup_label = QLabel("Startup: —")
        for lbl in [self.health_cpu_label, self.health_ram_label, self.health_disk_label, self.health_startup_label]:
            lbl.setStyleSheet("background: #11151d; border: 1px solid #2b3240; border-radius: 10px; padding: 12px;")
        info_row.addWidget(self.health_cpu_label, 0, 0)
        info_row.addWidget(self.health_ram_label, 0, 1)
        info_row.addWidget(self.health_disk_label, 1, 0)
        info_row.addWidget(self.health_startup_label, 1, 1)
        layout.addLayout(info_row)

        buttons = QHBoxLayout()
        self.btn_health_refresh = QPushButton("Обновить")
        self.btn_health_refresh.clicked.connect(self.refresh_health)

        self.btn_restore_point = QPushButton("Создать точку восстановления")
        self.btn_restore_point.clicked.connect(self.create_restore_point_now)

        self.btn_health_backup = QPushButton("Бэкап настроек")
        self.btn_health_backup.clicked.connect(self.do_backup_settings)

        buttons.addWidget(self.btn_health_refresh)
        buttons.addWidget(self.btn_restore_point)
        buttons.addWidget(self.btn_health_backup)
        buttons.addStretch()
        layout.addLayout(buttons)

        self.health_warnings = QListWidget()
        layout.addWidget(self.health_warnings)

        self.tabs.addTab(w, "Здоровье")

    def init_processes_tab(self):
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(12)

        title = QLabel("Управление процессами")
        title.setStyleSheet("font-size: 22px; font-weight: 800;")
        layout.addWidget(title)

        buttons = QHBoxLayout()
        self.btn_proc_refresh = QPushButton("Обновить")
        self.btn_proc_refresh.clicked.connect(self.refresh_processes)
        self.btn_proc_kill = QPushButton("Завершить выбранный")
        self.btn_proc_kill.clicked.connect(self.kill_selected_process)
        buttons.addWidget(self.btn_proc_refresh)
        buttons.addWidget(self.btn_proc_kill)
        buttons.addStretch()
        layout.addLayout(buttons)

        self.processes_list = QTreeWidget()
        self.processes_list.setHeaderLabels(["PID", "Имя", "CPU %", "Память", "Пользователь"])
        self.processes_list.header().setSectionResizeMode(0, QHeaderView.ResizeToContents)
        self.processes_list.header().setSectionResizeMode(1, QHeaderView.Stretch)
        self.processes_list.header().setSectionResizeMode(2, QHeaderView.ResizeToContents)
        self.processes_list.header().setSectionResizeMode(3, QHeaderView.ResizeToContents)
        self.processes_list.header().setSectionResizeMode(4, QHeaderView.Stretch)
        layout.addWidget(self.processes_list)

        self.tabs.addTab(w, "Процессы")

    def init_services_tab(self):
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(12)

        title = QLabel("Управление службами")
        title.setStyleSheet("font-size: 22px; font-weight: 800;")
        layout.addWidget(title)

        buttons = QHBoxLayout()
        self.btn_svc_refresh = QPushButton("Обновить")
        self.btn_svc_refresh.clicked.connect(self.refresh_services)
        self.btn_svc_start = QPushButton("Запустить")
        self.btn_svc_start.clicked.connect(lambda: self.control_selected_service("start"))
        self.btn_svc_stop = QPushButton("Остановить")
        self.btn_svc_stop.clicked.connect(lambda: self.control_selected_service("stop"))
        buttons.addWidget(self.btn_svc_refresh)
        buttons.addWidget(self.btn_svc_start)
        buttons.addWidget(self.btn_svc_stop)
        buttons.addStretch()
        layout.addLayout(buttons)

        self.services_list = QTreeWidget()
        self.services_list.setHeaderLabels(["Имя", "Отображаемое имя", "Статус", "Тип запуска"])
        self.services_list.header().setSectionResizeMode(0, QHeaderView.ResizeToContents)
        self.services_list.header().setSectionResizeMode(1, QHeaderView.Stretch)
        self.services_list.header().setSectionResizeMode(2, QHeaderView.ResizeToContents)
        self.services_list.header().setSectionResizeMode(3, QHeaderView.ResizeToContents)
        layout.addWidget(self.services_list)

        self.tabs.addTab(w, "Службы")

    def init_startup_tab(self):
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(12)

        title = QLabel("Управление автозагрузкой")
        title.setStyleSheet("font-size: 22px; font-weight: 800;")
        layout.addWidget(title)

        self.startup_filter = QLineEdit()
        self.startup_filter.setPlaceholderText("Поиск по автозагрузке...")
        self.startup_filter.textChanged.connect(self.filter_startup_items)
        layout.addWidget(self.startup_filter)

        self.startup_list = QTreeWidget()
        self.startup_list.setSelectionMode(QAbstractItemView.SingleSelection)
        self.startup_list.setHeaderLabels(["Тип", "Имя", "Источник", "Значение / Путь"])
        self.startup_list.header().setSectionResizeMode(0, QHeaderView.ResizeToContents)
        self.startup_list.header().setSectionResizeMode(1, QHeaderView.Stretch)
        self.startup_list.header().setSectionResizeMode(2, QHeaderView.Stretch)
        self.startup_list.header().setSectionResizeMode(3, QHeaderView.Stretch)
        layout.addWidget(self.startup_list)

        buttons = QHBoxLayout()
        self.btn_refresh_startup = QPushButton("Обновить список")
        self.btn_refresh_startup.clicked.connect(self.load_startup_items)
        self.btn_disable_startup = QPushButton("Отключить выбранное с резервной копией")
        self.btn_disable_startup.clicked.connect(self.disable_startup_selected)
        buttons.addWidget(self.btn_refresh_startup)
        buttons.addWidget(self.btn_disable_startup)
        buttons.addStretch()
        layout.addLayout(buttons)

        note = QLabel("Поддерживаются HKCU, HKLM (32/64-bit views) и папки Startup.")
        note.setStyleSheet("color: #aab3c2;")
        layout.addWidget(note)

        self.tabs.addTab(w, "Автозагрузка")

        self.load_startup_items()

    def init_privacy_tab(self):
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(12)

        title = QLabel("Конфиденциальность")
        title.setStyleSheet("font-size: 22px; font-weight: 800;")
        layout.addWidget(title)

        note = QLabel("Очистка кэшей браузеров, Internet cache и связанных временных данных.")
        note.setStyleSheet("color: #aab3c2;")
        layout.addWidget(note)

        self.btn_privacy_clean = QPushButton("Очистить приватные кэши")
        self.btn_privacy_clean.clicked.connect(self.run_privacy_cleanup)
        layout.addWidget(self.btn_privacy_clean)

        self.tabs.addTab(w, "Конфиденциальность")

    def init_games_tab(self):
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(12)

        title = QLabel("Очистка игровых кэшей")
        title.setStyleSheet("font-size: 22px; font-weight: 800;")
        layout.addWidget(title)

        note = QLabel("Steam, Discord, Epic, NVIDIA, D3D cache и похожие временные данные.")
        note.setStyleSheet("color: #aab3c2;")
        layout.addWidget(note)

        self.btn_games_clean = QPushButton("Очистить игровые кэши")
        self.btn_games_clean.clicked.connect(self.run_games_cleanup)
        layout.addWidget(self.btn_games_clean)

        self.tabs.addTab(w, "Игры")

    def init_scheduler_tab(self):
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(12)

        title = QLabel("Планировщик очистки")
        title.setStyleSheet("font-size: 22px; font-weight: 800;")
        layout.addWidget(title)

        row = QHBoxLayout()
        self.schedule_combo = QComboBox()
        self.schedule_combo.addItem("Ежедневно", "DAILY")
        self.schedule_combo.addItem("Еженедельно", "WEEKLY")
        self.schedule_combo.addItem("Ежемесячно", "MONTHLY")

        saved_sc = self.settings.get("cleanup_schedule", "WEEKLY")
        for i in range(self.schedule_combo.count()):
            if self.schedule_combo.itemData(i) == saved_sc:
                self.schedule_combo.setCurrentIndex(i)
                break

        self.schedule_time = QTimeEdit()
        t = QTime.fromString(self.settings.get("cleanup_time", "09:00"), "HH:mm")
        if not t.isValid():
            t = QTime(9, 0)
        self.schedule_time.setTime(t)
        self.schedule_time.setDisplayFormat("HH:mm")

        row.addWidget(QLabel("Период:"))
        row.addWidget(self.schedule_combo)
        row.addWidget(QLabel("Время:"))
        row.addWidget(self.schedule_time)
        row.addStretch()
        layout.addLayout(row)

        warn = QLabel(
            "Будет запускаться авто-очистка по отмеченным пунктам ниже. "
            "Если включены опасные операции, при необходимости создаётся точка восстановления."
        )
        warn.setWordWrap(True)
        warn.setStyleSheet("color: #aab3c2;")
        layout.addWidget(warn)

        grid = QGridLayout()
        self.schedule_checks = {}

        actions = [
            ("temp", "Временные файлы"),
            ("recycle", "Корзина"),
            ("thumbnails", "Кэш миниатюр"),
            ("logs", "Журналы"),
            ("dns", "DNS cache"),
            ("recent", "Недавние документы"),
            ("privacy", "Конфиденциальность"),
            ("games", "Игровые кэши"),
            ("updates", "Обновления Windows"),
            ("windows_old", "Windows.old"),
        ]
        default_checked = set(self.settings.get("scheduled_actions", DEFAULT_SCHEDULED_ACTIONS))

        for idx, (key, text) in enumerate(actions):
            cb = QCheckBox(text)
            cb.setChecked(key in default_checked)
            self.schedule_checks[key] = cb
            grid.addWidget(cb, idx // 2, idx % 2)

        layout.addLayout(grid)

        buttons = QHBoxLayout()
        self.btn_create_task = QPushButton("Создать задачу")
        self.btn_create_task.clicked.connect(self.create_scheduled_task)
        self.btn_delete_task = QPushButton("Удалить задачу")
        self.btn_delete_task.clicked.connect(self.delete_scheduled_task)
        buttons.addWidget(self.btn_create_task)
        buttons.addWidget(self.btn_delete_task)
        buttons.addStretch()
        layout.addLayout(buttons)

        self.tabs.addTab(w, "Планировщик")

    def init_updates_tab(self):
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(12)

        title = QLabel("Обновления программы")
        title.setStyleSheet("font-size: 22px; font-weight: 800;")
        layout.addWidget(title)

        self.lbl_current_version = QLabel(f"Текущая версия: {APP_VERSION}")
        self.lbl_update_status = QLabel("Статус: не проверялось")
        self.lbl_current_version.setStyleSheet("font-size: 14px;")
        self.lbl_update_status.setStyleSheet("color: #aab3c2;")
        layout.addWidget(self.lbl_current_version)
        layout.addWidget(self.lbl_update_status)

        self.update_notes = QTextEdit()
        self.update_notes.setReadOnly(True)
        self.update_notes.setPlainText("Нажмите «Проверить обновления».")
        layout.addWidget(self.update_notes)

        buttons = QHBoxLayout()
        self.btn_update_check = QPushButton("Проверить обновления")
        self.btn_update_check.clicked.connect(self.check_updates)
        self.btn_update_open = QPushButton("Открыть страницу")
        self.btn_update_open.clicked.connect(self.open_update_url)
        buttons.addWidget(self.btn_update_check)
        buttons.addWidget(self.btn_update_open)
        buttons.addStretch()
        layout.addLayout(buttons)

        self.tabs.addTab(w, "Обновления")

    def init_settings_tab(self):
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(12)

        title = QLabel("Настройки")
        title.setStyleSheet("font-size: 22px; font-weight: 800;")
        layout.addWidget(title)

        self.spin_big_files_min = QSpinBox()
        self.spin_big_files_min.setRange(1, 5000)
        self.spin_big_files_min.setValue(int(self.settings.get("big_files_min_mb", 100)))
        self.spin_big_files_min.setSuffix(" МБ")

        self.spin_duplicate_min = QSpinBox()
        self.spin_duplicate_min.setRange(1, 5000)
        self.spin_duplicate_min.setValue(int(self.settings.get("duplicate_min_mb", 10)))
        self.spin_duplicate_min.setSuffix(" МБ")

        self.chk_check_updates_on_start = QCheckBox("Проверять обновления при запуске")
        self.chk_check_updates_on_start.setChecked(bool(self.settings.get("check_updates_on_start", True)))

        grid = QGridLayout()
        grid.addWidget(QLabel("Порог больших файлов:"), 0, 0)
        grid.addWidget(self.spin_big_files_min, 0, 1)
        grid.addWidget(QLabel("Порог дубликатов:"), 1, 0)
        grid.addWidget(self.spin_duplicate_min, 1, 1)
        grid.addWidget(self.chk_check_updates_on_start, 2, 0, 1, 2)

        note_restore = QLabel(
            "Опция создания точки восстановления находится на вкладке «Очистка»."
        )
        note_restore.setStyleSheet("color: #aab3c2;")
        grid.addWidget(note_restore, 3, 0, 1, 2)

        layout.addLayout(grid)

        note = QLabel("Резервная копия настроек доступна через кнопку ниже.")
        note.setStyleSheet("color: #aab3c2;")
        layout.addWidget(note)

        buttons = QHBoxLayout()
        self.btn_settings_save = QPushButton("Сохранить настройки")
        self.btn_settings_save.clicked.connect(lambda: self.save_ui_settings(silent=False))
        self.btn_settings_backup = QPushButton("Создать резервную копию")
        self.btn_settings_backup.clicked.connect(self.do_backup_settings)
        buttons.addWidget(self.btn_settings_save)
        buttons.addWidget(self.btn_settings_backup)
        buttons.addStretch()
        layout.addLayout(buttons)

        self.tabs.addTab(w, "Настройки")

    def refresh_dashboard(self):
        snap = system_health_snapshot()
        self.lbl_dash_health.setText(f"{snap['score']}/100")
        self.dashboard_health_bar.setValue(snap["score"])

        if snap["state"]:
            self.card_health.setToolTip("\n".join(snap.get("warnings", [])) or snap["state"])

        if snap.get("disk_free") is not None and snap.get("disk_total") is not None:
            self.lbl_dash_disk.setText(format_size(snap["disk_free"]))
        else:
            self.lbl_dash_disk.setText("N/A")

        if snap.get("mem_percent") is not None:
            self.lbl_dash_ram.setText(f"{snap['mem_percent']:.0f}%")
        else:
            self.lbl_dash_ram.setText("N/A")

        if snap.get("cpu_percent") is not None:
            self.lbl_dash_cpu.setText(f"{snap['cpu_percent']:.0f}%")
        else:
            self.lbl_dash_cpu.setText("N/A")

        self.lbl_dash_startup.setText(str(snap.get("startup_count", 0)))

        if self.update_info and self.update_info.get("available"):
            remote = self.update_info.get("remote_version", "?")
            self.lbl_dash_update.setText(f"Есть {remote}")
        elif self.update_info:
            self.lbl_dash_update.setText("Актуально")
        else:
            self.lbl_dash_update.setText("Не проверено")

        last_cleanup = self.settings.get("last_cleanup", "")
        last_freed = self.settings.get("last_freed", 0)
        if last_cleanup:
            self.lbl_last_cleanup.setText(
                f"Последняя очистка: {last_cleanup} | Освобождено: {format_size(last_freed)}"
            )
        else:
            self.lbl_last_cleanup.setText("Последняя очистка: —")

    def on_tab_changed(self, index):
        name = self.tabs.tabText(index)

        if name == "Dashboard":
            self.refresh_dashboard()
        elif name == "Здоровье":
            self.refresh_health()
        elif name == "Процессы":
            self.refresh_processes()
        elif name == "Службы":
            self.refresh_services()
        elif name == "Автозагрузка":
            self.load_startup_items()
        elif name == "Обновления":
            if self.update_info is None and self.settings.get("check_updates_on_start", True):
                self.check_updates()

    def start_system_monitor(self):
        try:
            import psutil
            self.use_psutil = True
            try:
                psutil.cpu_percent(interval=None)
            except Exception:
                pass
        except Exception:
            self.use_psutil = False
            try:
                import wmi
                self.wmi = wmi.WMI()
            except Exception:
                self.wmi = None

        self.update_system_info()

    def update_system_info(self):
        try:
            if self.use_psutil:
                import psutil

                cpu_load = psutil.cpu_percent(interval=None)
                self.safe_status(
                    f"CPU {cpu_load:.0f}% | RAM {psutil.virtual_memory().percent:.0f}% | Disk OK"
                )

            else:
                if self.wmi:
                    cpu = self.wmi.Win32_Processor()[0]
                    self.safe_status(f"Процессор: {cpu.Name}")
                else:
                    self.safe_status("Мониторинг: недоступно")
        except Exception:
            logging.exception("Ошибка обновления мониторинга")

    def start_analysis(self):
        if self.analysis_thread and self.analysis_thread.isRunning():
            return
        if self.fix_thread and self.fix_thread.isRunning():
            return

        self.issues_list.setEnabled(False)
        self.issues_list.clear()
        self.issues = []
        self.progress_label.setText("Сканирование системы...")
        self.progress_label.setStyleSheet("color: #ffaa00;")
        self.fix_status.setText("")
        self.btn_scan.setEnabled(False)
        self.btn_fix.setEnabled(False)
        self.cleanup_progress.setRange(0, 0)

        self.safe_status("Запущен анализ системы")
        logging.info("Пользователь запустил анализ")

        self.analysis_thread = AnalysisThread()
        self.analysis_thread.progress.connect(self.update_analysis_progress)
        self.analysis_thread.analysisFinished.connect(self.on_analysis_finished)
        self.analysis_thread.start()

    def update_analysis_progress(self, msg):
        self.progress_label.setText(msg)
        self.safe_status(msg)

    def on_analysis_finished(self, issues):
        self.issues = issues
        self.issues_list.clear()
        self.issues_list.setEnabled(True)

        if not issues:
            self.progress_label.setText("Система чиста, мусор не найден.")
            self.progress_label.setStyleSheet("color: #00cc66;")
            self.btn_fix.setEnabled(False)
            self.safe_status("Анализ завершён: проблем не найдено")
        else:
            total_size = 0
            for issue in issues:
                if issue["size"]:
                    total_size += issue["size"]

                item = QListWidgetItem()
                widget = QWidget()
                layout = QHBoxLayout(widget)
                layout.setContentsMargins(6, 3, 6, 3)

                checkbox = QCheckBox()
                checkbox.setChecked(issue["checked"])
                checkbox.setProperty("issue_id", issue["id"])
                checkbox.stateChanged.connect(self.update_fix_button_state)

                prefix = "🔴 " if issue.get("dangerous") else "🟢 "
                label_text = f"{prefix}{issue['category']}: {issue['description']}"
                if issue["size"] is not None:
                    label_text += f" ({issue['size_str']})"

                label = QLabel(label_text)

                layout.addWidget(checkbox)
                layout.addWidget(label)
                layout.addStretch()
                widget.setStyleSheet("background: transparent;")

                item.setSizeHint(widget.sizeHint())
                self.issues_list.addItem(item)
                self.issues_list.setItemWidget(item, widget)

            self.progress_label.setText(
                f"Найдено {len(issues)} проблем. "
                f"Примерно можно освободить {format_size(total_size)}."
            )
            self.safe_status(f"Анализ завершён: найдено {len(issues)} проблем")
            self.update_fix_button_state()

        self.btn_scan.setEnabled(True)
        self.cleanup_progress.setRange(0, 1)
        self.cleanup_progress.setValue(0)
        self.analysis_thread = None
        self.refresh_dashboard()

    def get_checked_issues(self):
        selected = []
        for i in range(self.issues_list.count()):
            item = self.issues_list.item(i)
            widget = self.issues_list.itemWidget(item)
            if widget:
                checkbox = widget.findChild(QCheckBox)
                if checkbox and checkbox.isChecked():
                    issue_id = checkbox.property("issue_id")
                    for issue in self.issues:
                        if issue["id"] == issue_id:
                            selected.append(issue)
                            break
        return selected

    def update_fix_button_state(self, *args):
        self.btn_fix.setEnabled(bool(self.get_checked_issues()))

    def start_fix(self):
        if self.fix_thread and self.fix_thread.isRunning():
            return
        if self.analysis_thread and self.analysis_thread.isRunning():
            return

        selected_issues = self.get_checked_issues()
        if not selected_issues:
            QMessageBox.information(self, "Внимание", "Не выбрано ни одного действия.")
            return

        selected_actions = []
        total_size = 0
        dangerous_selected = False

        for issue in selected_issues:
            selected_actions.append((issue["description"], issue["action"]))
            if issue["size"] is not None:
                total_size += issue["size"]
            if issue.get("dangerous"):
                dangerous_selected = True

        size_str = format_size(total_size) if total_size > 0 else "неизвестно"

        msg = (
            f"Будет удалено примерно {size_str} данных.\n"
            f"Будет выполнено действий: {len(selected_actions)}.\n\n"
            f"Продолжить?"
        )

        if dangerous_selected:
            msg += (
                "\n\nВнимание: среди выбранных действий есть потенциально опасные операции "
                "(например, обновления Windows или Windows.old)."
            )

        reply = QMessageBox.question(self, "Подтверждение", msg, QMessageBox.Yes | QMessageBox.No)
        if reply != QMessageBox.Yes:
            return

        if dangerous_selected and self.chk_auto_restore_point.isChecked():
            self.safe_status("Создание точки восстановления...")
            QApplication.processEvents()
            create_restore_point("PC Optimizer before dangerous cleanup")

        self.issues_list.setEnabled(False)
        self.fix_status.setText("Выполняется очистка...")
        self.fix_status.setStyleSheet("color: #ffaa00;")
        self.btn_fix.setEnabled(False)
        self.btn_scan.setEnabled(False)
        self.cleanup_progress.setRange(0, 0)

        self.safe_status("Запущена очистка")
        logging.info("Пользователь запустил очистку выбранных объектов")

        self.fix_thread = FixThread(selected_actions)
        self.fix_thread.progress.connect(self.update_fix_progress)
        self.fix_thread.fixFinished.connect(self.on_fix_finished)
        self.fix_thread.start()

    def start_direct_cleanup(self, description, action):
        if self.any_worker_running():
            return

        self.issues_list.setEnabled(False)
        self.fix_status.setText(f"Выполняется: {description}")
        self.btn_fix.setEnabled(False)
        self.btn_scan.setEnabled(False)
        self.cleanup_progress.setRange(0, 0)

        self.safe_status(f"Запущена операция: {description}")
        self.fix_thread = FixThread([(description, action)])
        self.fix_thread.progress.connect(self.update_fix_progress)
        self.fix_thread.fixFinished.connect(self.on_fix_finished)
        self.fix_thread.start()

    def update_fix_progress(self, msg):
        self.fix_status.setText(msg)
        self.safe_status(msg)

    def on_fix_finished(self, stats):
        deleted = stats.get("deleted", 0)
        locked = stats.get("locked", 0)
        access_denied = stats.get("access_denied", 0)
        other_errors = stats.get("other_errors", 0)
        bytes_freed = stats.get("bytes_freed", 0)
        total_issues = locked + access_denied + other_errors

        self.issues_list.setEnabled(True)

        if deleted == 0 and total_issues > 0:
            msg = (
                "Очистка не смогла удалить выбранные объекты.\n"
                "Возможно, они используются системой или требуют дополнительных прав."
            )
        else:
            msg = (
                f"Удалено объектов: {deleted}\n"
                f"Освобождено: {format_size(bytes_freed)}"
            )

            details = []
            if locked:
                details.append(f"занято системой: {locked}")
            if access_denied:
                details.append(f"нет доступа: {access_denied}")
            if other_errors:
                details.append(f"прочие ошибки: {other_errors}")

            if details:
                msg += "\nПропущено:\n" + "\n".join(details)

        if total_issues > 0:
            QMessageBox.information(self, "Очистка завершена", msg)
            self.fix_status.setText(f"Очистка завершена с пропусками (пропущено: {total_issues})")
            self.safe_status(f"Очистка завершена с пропусками: {total_issues}")
        else:
            QMessageBox.information(self, "Успех", msg)
            self.fix_status.setText(
                f"Очистка завершена успешно (удалено: {deleted}, освобождено: {format_size(bytes_freed)})"
            )
            self.safe_status(f"Очистка завершена успешно: {deleted}")

        self.btn_fix.setEnabled(False)
        self.btn_scan.setEnabled(True)
        self.progress_label.setText("Очистка завершена. Запустите анализ снова для обновления списка.")
        self.cleanup_progress.setRange(0, 1)
        self.cleanup_progress.setValue(0)
        self.fix_thread = None

        self.settings["last_cleanup"] = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        self.settings["last_freed"] = bytes_freed
        save_settings(self.settings)
        self.refresh_dashboard()

    def start_big_files_scan(self):
        if self.big_files_thread and self.big_files_thread.isRunning():
            return

        roots = self.collect_roots_from_edit(self.edit_big_roots, get_user_profile_dir())
        if not roots:
            QMessageBox.information(self, "Внимание", "Не выбрано ни одной папки.")
            return

        self.big_files_list.clear()
        self.btn_big_scan.setEnabled(False)
        self.big_files_thread = BigFilesThread(roots, self.spin_big_files_min.value())
        self.big_files_thread.progress.connect(self.safe_status)
        self.big_files_thread.finishedData.connect(self.on_big_files_found)
        self.big_files_thread.start()

    def on_big_files_found(self, items):
        self.big_files_list.clear()
        for x in items:
            item = QTreeWidgetItem([x["path"], x["size_str"]])
            item.setData(0, Qt.UserRole, x["path"])
            self.big_files_list.addTopLevelItem(item)
        self.btn_big_scan.setEnabled(True)
        self.safe_status(f"Найдено больших файлов: {len(items)}")
        self.big_files_thread = None

    def start_duplicates_scan(self):
        if self.duplicates_thread and self.duplicates_thread.isRunning():
            return

        roots = self.collect_roots_from_edit(self.edit_dup_roots, get_user_profile_dir())
        if not roots:
            QMessageBox.information(self, "Внимание", "Не выбрано ни одной папки.")
            return

        self.dup_list.clear()
        self.btn_dup_scan.setEnabled(False)
        self.duplicates_thread = DuplicatesThread(roots, self.spin_duplicate_min.value())
        self.duplicates_thread.progress.connect(self.safe_status)
        self.duplicates_thread.finishedData.connect(self.on_duplicates_found)
        self.duplicates_thread.start()

    def on_duplicates_found(self, groups):
        self.dup_list.clear()
        for group in groups:
            root_item = QTreeWidgetItem([f"Дубликаты: {group['count']} файлов", group["size_str"]])
            root_item.setData(0, Qt.UserRole, None)
            for path in group["files"]:
                child = QTreeWidgetItem([path, group["size_str"]])
                child.setData(0, Qt.UserRole, path)
                root_item.addChild(child)
            self.dup_list.addTopLevelItem(root_item)
            root_item.setExpanded(True)

        self.btn_dup_scan.setEnabled(True)
        self.safe_status(f"Найдено групп дубликатов: {len(groups)}")
        self.duplicates_thread = None

    def populate_drive_combo(self):
        self.disk_drive_combo.clear()
        drives = get_drive_letters()
        for d in drives:
            self.disk_drive_combo.addItem(d, d)
        if not drives:
            self.disk_drive_combo.addItem(get_system_drive_root(), get_system_drive_root())

    def start_disk_map_scan(self):
        if self.disk_map_thread and self.disk_map_thread.isRunning():
            return

        root = self.disk_drive_combo.currentData() or get_system_drive_root()
        self.btn_disk_scan.setEnabled(False)
        self.disk_map_list.clear()
        self.disk_map_thread = DiskMapThread([root])
        self.disk_map_thread.progress.connect(self.safe_status)
        self.disk_map_thread.finishedData.connect(self.on_disk_map_ready)
        self.disk_map_thread.start()

    def on_disk_map_ready(self, items):
        self.disk_map_list.clear()
        for x in items:
            pct = f"{x['percent']:.1f}%"
            item = QTreeWidgetItem([x["name"], x["size_str"], pct])
            item.setData(0, Qt.UserRole, x["path"])
            self.disk_map_list.addTopLevelItem(item)

        self.btn_disk_scan.setEnabled(True)
        self.safe_status(f"Карта диска построена: {len(items)} элементов")
        self.disk_map_thread = None

    def refresh_health(self):
        if self.health_thread and self.health_thread.isRunning():
            return

        self.btn_health_refresh.setEnabled(False)
        self.health_thread = HealthThread()
        self.health_thread.progress.connect(self.safe_status)
        self.health_thread.finishedData.connect(self.on_health_ready)
        self.health_thread.start()

    def on_health_ready(self, snap):
        self.btn_health_refresh.setEnabled(True)

        self.health_score_label.setText(f"Здоровье: {snap.get('score', 0)}/100")
        self.health_state_label.setText(f"Статус: {snap.get('state', '—')}")
        self.health_progress.setValue(int(snap.get("score", 0)))

        cpu = snap.get("cpu_percent")
        memp = snap.get("mem_percent")
        diskf = snap.get("disk_free")
        diskt = snap.get("disk_total")
        startup = snap.get("startup_count", 0)

        self.health_cpu_label.setText(f"CPU: {cpu:.0f}%" if cpu is not None else "CPU: N/A")
        if memp is not None and snap.get("mem_used") is not None and snap.get("mem_total") is not None:
            self.health_ram_label.setText(
                f"RAM: {memp:.0f}% ({format_size(snap['mem_used'])} / {format_size(snap['mem_total'])})"
            )
        else:
            self.health_ram_label.setText("RAM: N/A")

        if diskf is not None and diskt is not None and snap.get("disk_used") is not None:
            self.health_disk_label.setText(
                f"Disk: {format_size(diskf)} free ({format_size(snap['disk_used'])} / {format_size(diskt)})"
            )
        else:
            self.health_disk_label.setText("Disk: N/A")

        self.health_startup_label.setText(f"Startup: {startup}")

        self.health_warnings.clear()
        warnings = snap.get("warnings", [])
        if warnings:
            for w in warnings:
                self.health_warnings.addItem(QListWidgetItem("⚠ " + w))
        else:
            self.health_warnings.addItem(QListWidgetItem("Серьёзных предупреждений не найдено."))

        self.safe_status(f"Здоровье системы: {snap.get('state', '—')}")

    def refresh_processes(self):
        if self.processes_thread and self.processes_thread.isRunning():
            return

        self.btn_proc_refresh.setEnabled(False)
        self.processes_list.clear()
        self.processes_thread = ProcessesThread()
        self.processes_thread.progress.connect(self.safe_status)
        self.processes_thread.finishedData.connect(self.on_processes_ready)
        self.processes_thread.start()

    def on_processes_ready(self, rows):
        self.processes_list.clear()
        for p in rows:
            item = QTreeWidgetItem([
                str(p["pid"]),
                p["name"],
                f"{p['cpu']:.1f}",
                p["memory_str"],
                p["user"]
            ])
            item.setData(0, Qt.UserRole, p["pid"])
            self.processes_list.addTopLevelItem(item)
        self.btn_proc_refresh.setEnabled(True)
        self.safe_status(f"Процессы загружены: {len(rows)}")
        self.processes_thread = None

    def kill_selected_process(self):
        item = self.processes_list.currentItem()
        if not item:
            QMessageBox.information(self, "Внимание", "Выберите процесс.")
            return

        pid = item.data(0, Qt.UserRole)
        name = item.text(1)

        reply = QMessageBox.question(
            self,
            "Подтверждение",
            f"Завершить процесс?\n\n{name} (PID {pid})",
            QMessageBox.Yes | QMessageBox.No
        )
        if reply != QMessageBox.Yes:
            return

        if terminate_process(pid):
            self.refresh_processes()
            self.safe_status(f"Процесс завершён: {name}")
        else:
            QMessageBox.warning(self, "Ошибка", "Не удалось завершить процесс.")

    def refresh_services(self):
        if self.services_thread and self.services_thread.isRunning():
            return

        self.btn_svc_refresh.setEnabled(False)
        self.services_list.clear()
        self.services_thread = ServicesThread()
        self.services_thread.progress.connect(self.safe_status)
        self.services_thread.finishedData.connect(self.on_services_ready)
        self.services_thread.start()

    def on_services_ready(self, rows):
        self.services_list.clear()
        for s in rows:
            item = QTreeWidgetItem([
                s.get("name", ""),
                s.get("display_name", ""),
                s.get("status", ""),
                s.get("start_type", "")
            ])
            item.setData(0, Qt.UserRole, s.get("name"))
            self.services_list.addTopLevelItem(item)
        self.btn_svc_refresh.setEnabled(True)
        self.safe_status(f"Службы загружены: {len(rows)}")
        self.services_thread = None

    def control_selected_service(self, action):
        item = self.services_list.currentItem()
        if not item:
            QMessageBox.information(self, "Внимание", "Выберите службу.")
            return

        name = item.data(0, Qt.UserRole)
        if not name:
            return

        reply = QMessageBox.question(
            self,
            "Подтверждение",
            f"{'Запустить' if action == 'start' else 'Остановить'} службу?\n\n{name}",
            QMessageBox.Yes | QMessageBox.No
        )
        if reply != QMessageBox.Yes:
            return

        if control_service(name, action):
            self.refresh_services()
            self.safe_status(f"Служба: {action} — {name}")
        else:
            QMessageBox.warning(self, "Ошибка", "Не удалось выполнить операцию.")

    def load_startup_items(self):
        self.startup_list.clear()

        if winreg is None:
            self.startup_list.addTopLevelItem(QTreeWidgetItem(["—", "Автозагрузка доступна только в Windows.", "", ""]))
            self.btn_disable_startup.setEnabled(False)
            self.btn_refresh_startup.setEnabled(False)
            return

        items = collect_startup_items()

        if items:
            for data in items:
                item = QTreeWidgetItem([
                    data.get("display_type", ""),
                    data.get("display_name", ""),
                    data.get("display_source", ""),
                    data.get("display_value", ""),
                ])
                item.setData(0, Qt.UserRole, data)
                if data.get("tooltip"):
                    item.setToolTip(0, data["tooltip"])
                    item.setToolTip(1, data["tooltip"])
                    item.setToolTip(2, data["tooltip"])
                    item.setToolTip(3, data["tooltip"])
                self.startup_list.addTopLevelItem(item)

            self.btn_disable_startup.setEnabled(True)
            self.safe_status("Список автозагрузки обновлён")
        else:
            self.startup_list.addTopLevelItem(QTreeWidgetItem(["—", "Записей автозагрузки не найдено.", "", ""]))
            self.btn_disable_startup.setEnabled(False)
            self.safe_status("Автозагрузка: записей не найдено")

        self.btn_refresh_startup.setEnabled(True)
        self.filter_startup_items(self.startup_filter.text())

    def filter_startup_items(self, text):
        text = (text or "").lower().strip()
        for i in range(self.startup_list.topLevelItemCount()):
            item = self.startup_list.topLevelItem(i)
            match = not text or any(text in item.text(c).lower() for c in range(4))
            item.setHidden(not match)

    def disable_startup_selected(self):
        if winreg is None:
            QMessageBox.warning(self, "Внимание", "Автозагрузка доступна только в Windows.")
            return

        item = self.startup_list.currentItem()
        if not item:
            QMessageBox.information(self, "Внимание", "Выберите элемент автозагрузки.")
            return

        data = item.data(0, Qt.UserRole) or {}
        kind = data.get("kind")

        if kind == "registry":
            name = data.get("name")
            root_kind = data.get("root_kind")
            view_flag = data.get("view_flag", 0)
            reg_type = data.get("reg_type")
            value = data.get("value")

            if not name or not root_kind:
                QMessageBox.critical(self, "Ошибка", "Не удалось определить запись автозагрузки.")
                return

            reply = QMessageBox.question(
                self,
                "Подтверждение",
                f"Отключить автозагрузку для:\n{name}\n\nБудет создана резервная копия ключа Run.",
                QMessageBox.Yes | QMessageBox.No
            )
            if reply != QMessageBox.Yes:
                return

            try:
                timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
                safe_name = sanitize_filename(name)
                backup_path = os.path.join(
                    get_desktop_dir(),
                    f"startup_backup_{root_kind}_{safe_name}_{timestamp}.reg"
                )

                reg_path = startup_reg_export_path(root_kind, view_flag)
                content = build_startup_backup_content(reg_path, name, value, reg_type)
                with open(backup_path, "w", encoding="utf-16") as f:
                    f.write(content)

                root = startup_reg_open_root(root_kind)
                access = winreg.KEY_SET_VALUE | view_flag
                with winreg.OpenKey(root, STARTUP_RUN_PATH, 0, access) as key:
                    winreg.DeleteValue(key, name)

                self.load_startup_items()
                QMessageBox.information(
                    self,
                    "Успех",
                    f"Автозагрузка отключена:\n{name}\n\nРезервная копия:\n{backup_path}"
                )
                self.safe_status(f"Автозагрузка отключена: {name}")
            except Exception as e:
                logging.exception(f"Ошибка отключения автозагрузки для {name}")
                QMessageBox.critical(self, "Ошибка", f"Не удалось отключить:\n{e}")

        elif kind == "startup_file":
            source_path = data.get("path")
            if not source_path:
                QMessageBox.critical(self, "Ошибка", "Не удалось определить файл автозагрузки.")
                return

            if not os.path.exists(source_path):
                QMessageBox.warning(self, "Внимание", "Файл уже отсутствует.")
                self.load_startup_items()
                return

            reply = QMessageBox.question(
                self,
                "Подтверждение",
                f"Отключить файл автозагрузки?\n\n{source_path}\n\nФайл будет перемещён в резервную папку.",
                QMessageBox.Yes | QMessageBox.No
            )
            if reply != QMessageBox.Yes:
                return

            try:
                backup_dir = os.path.join(get_desktop_dir(), "startup_backup")
                os.makedirs(backup_dir, exist_ok=True)

                timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
                base_name = os.path.basename(source_path)
                backup_path = os.path.join(backup_dir, f"{timestamp}_{base_name}")

                shutil.move(source_path, backup_path)

                self.load_startup_items()
                QMessageBox.information(
                    self,
                    "Успех",
                    f"Файл автозагрузки перемещён в резервную папку:\n{backup_path}"
                )
                self.safe_status(f"Автозагрузка отключена: {base_name}")
            except Exception as e:
                logging.exception(f"Ошибка отключения файла автозагрузки: {source_path}")
                QMessageBox.critical(self, "Ошибка", f"Не удалось отключить:\n{e}")

        else:
            QMessageBox.information(self, "Внимание", "Для этого элемента отключение не поддерживается.")

    def run_privacy_cleanup(self):
        reply = QMessageBox.question(
            self,
            "Подтверждение",
            "Очистить приватные кэши браузеров и Internet cache?",
            QMessageBox.Yes | QMessageBox.No
        )
        if reply != QMessageBox.Yes:
            return
        self.start_direct_cleanup("Конфиденциальность", clean_privacy)

    def run_games_cleanup(self):
        reply = QMessageBox.question(
            self,
            "Подтверждение",
            "Очистить игровые кэши (Steam / Discord / Epic / NVIDIA)?",
            QMessageBox.Yes | QMessageBox.No
        )
        if reply != QMessageBox.Yes:
            return
        self.start_direct_cleanup("Игровые кэши", clean_game_caches)

    def create_scheduled_task(self):
        self.save_ui_settings(silent=True)
        schedule = self.schedule_combo.currentData()
        time_str = self.schedule_time.time().toString("HH:mm")

        if create_cleanup_task(schedule, time_str):
            QMessageBox.information(
                self,
                "Планировщик",
                f"Задача создана:\n{TASK_NAME}\n\nПериод: {schedule}\nВремя: {time_str}"
            )
            self.safe_status("Задача планировщика создана")
        else:
            QMessageBox.warning(self, "Ошибка", "Не удалось создать задачу планировщика.")

    def delete_scheduled_task(self):
        reply = QMessageBox.question(
            self,
            "Подтверждение",
            f"Удалить задачу планировщика?\n\n{TASK_NAME}",
            QMessageBox.Yes | QMessageBox.No
        )
        if reply != QMessageBox.Yes:
            return

        if delete_cleanup_task():
            QMessageBox.information(self, "Планировщик", "Задача удалена.")
            self.safe_status("Задача планировщика удалена")
        else:
            QMessageBox.warning(self, "Ошибка", "Не удалось удалить задачу планировщика.")

    def check_updates(self):
        if self.update_thread and self.update_thread.isRunning():
            return

        self.btn_update_check.setEnabled(False)
        self.lbl_update_status.setText("Статус: проверка...")
        self.update_notes.setPlainText("Идёт проверка обновлений...")

        self.update_thread = UpdateCheckThread()
        self.update_thread.progress.connect(self.safe_status)
        self.update_thread.finishedData.connect(self.on_update_check_finished)
        self.update_thread.start()

    def open_update_url(self):
        if self.update_info and self.update_info.get("url"):
            webbrowser.open(self.update_info["url"])
        else:
            QMessageBox.information(self, "Обновления", "Сначала выполните проверку обновлений.")

    def on_update_check_finished(self, info):
        self.update_info = info
        self.btn_update_check.setEnabled(True)

        if info.get("available"):
            remote = info.get("remote_version", "?")
            notes = info.get("notes", "")
            url = info.get("url", "")

            self.lbl_update_status.setText(f"Статус: доступна новая версия {remote}")
            self.update_notes.setPlainText(
                f"Доступна новая версия: {remote}\n\nИзменения:\n{notes}\n\nURL:\n{url or '—'}"
            )

            msg = (
                f"Доступна новая версия: {remote}\n\n"
                f"{notes}\n\n"
                f"Открыть страницу загрузки?"
            )
            reply = QMessageBox.question(self, "Обновление найдено", msg, QMessageBox.Yes | QMessageBox.No)
            if reply == QMessageBox.Yes and url:
                webbrowser.open(url)
        else:
            self.lbl_update_status.setText("Статус: актуально или проверить не удалось")
            self.update_notes.setPlainText("Установлена актуальная версия или проверка не удалась.")
            QMessageBox.information(
                self,
                "Обновления",
                "У вас установлена актуальная версия или проверить обновления не удалось."
            )
        self.safe_status("Проверка обновлений завершена")

    def closeEvent(self, event):
        if self.any_worker_running():
            QMessageBox.warning(
                self,
                "Идёт операция",
                "Дождитесь завершения текущих операций перед закрытием программы."
            )
            event.ignore()
            return
        event.accept()


# ------------------------------------------------------------
# Запуск
# ------------------------------------------------------------
def relaunch_as_admin():
    if getattr(sys, "frozen", False):
        target = sys.executable
        params = subprocess.list2cmdline(["--elevated"] + sys.argv[1:])
    else:
        target = sys.executable
        script = os.path.abspath(sys.argv[0])
        params = subprocess.list2cmdline([script, "--elevated"] + sys.argv[1:])

    return ctypes.windll.shell32.ShellExecuteW(
        None,
        "runas",
        target,
        params,
        None,
        1
    )


if __name__ == "__main__":
    if os.name != "nt":
        print("Это приложение работает только в Windows.")
        sys.exit(1)

    def is_admin():
        try:
            return bool(ctypes.windll.shell32.IsUserAnAdmin())
        except Exception:
            return False

    if "--elevated" not in sys.argv and not is_admin():
        logging.info("Запрошено повышение прав администратора")
        rc = relaunch_as_admin()
        if rc <= 32:
            logging.error("Не удалось запустить приложение с правами администратора")
            sys.exit(1)
        sys.exit(0)

    if "--auto-clean" in sys.argv:
        sys.exit(run_headless_auto_cleanup())

    argv = [a for a in sys.argv if a != "--elevated"]
    app = QApplication(argv)
    app.setStyle("Fusion")

    window = PCOptimizer()
    window.show()

    sys.exit(app.exec_())
