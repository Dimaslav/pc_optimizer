# -*- coding: utf-8 -*-

import base64
import csv
import ctypes
import hashlib
import json
import logging
import os
import re
import shutil
import string
import subprocess
import tempfile
import time

from datetime import datetime

try:
    import winreg
except ImportError:
    winreg = None

try:
    import psutil
except ImportError:
    psutil = None

from core import (
    APP_NAME,
    APP_VERSION,
    REPORT_DIR,
    STARTUP_BACKUP_DIR,
    STARTUP_BACKUP_FILE,
    Candidate,
    OperationResult,
    format_size,
    get_system_drive,
    get_user_profile,
    is_reparse_point,
    normalize_path,
    normalize_paths,
    safe_filename,
    load_json,
    save_json,
    validate_delete_path,
)


STARTUP_RUN_PATH = (
    r"Software\Microsoft\Windows\CurrentVersion\Run"
)

CATEGORY_NAMES = {
    "temp": "Временные файлы",
    "thumbnails": "Кэш миниатюр",
    "privacy": "Кэши браузеров",
    "games": "Игровые кэши",
    "recent": "Недавние документы",
}


def run_command(arguments, timeout=60):
    try:
        result = subprocess.run(
            arguments,
            capture_output=True,
            text=True,
            shell=False,
            timeout=timeout,
            errors="replace",
        )

        return {
            "success": result.returncode == 0,
            "returncode": result.returncode,
            "stdout": result.stdout or "",
            "stderr": result.stderr or "",
        }

    except subprocess.TimeoutExpired:
        return {
            "success": False,
            "returncode": -1,
            "stdout": "",
            "stderr": "Превышено время ожидания",
        }

    except Exception as error:
        logging.exception("Ошибка команды: %r", arguments)

        return {
            "success": False,
            "returncode": -1,
            "stdout": "",
            "stderr": str(error),
        }


# ============================================================
# Каталоги очистки
# ============================================================

def temp_roots():
    return normalize_paths([
        tempfile.gettempdir(),
        os.environ.get("TEMP", ""),
        os.environ.get("TMP", ""),
    ])


def thumbnail_roots():
    return normalize_paths([
        os.path.join(
            get_user_profile(),
            "AppData",
            "Local",
            "Microsoft",
            "Windows",
            "Explorer",
        )
    ])


def recent_roots():
    return normalize_paths([
        os.path.join(
            get_user_profile(),
            "AppData",
            "Roaming",
            "Microsoft",
            "Windows",
            "Recent",
        )
    ])


def browser_cache_roots():
    user = get_user_profile()
    result = []

    bases = [
        os.path.join(
            user,
            "AppData",
            "Local",
            "Google",
            "Chrome",
            "User Data",
        ),
        os.path.join(
            user,
            "AppData",
            "Local",
            "Microsoft",
            "Edge",
            "User Data",
        ),
        os.path.join(
            user,
            "AppData",
            "Local",
            "BraveSoftware",
            "Brave-Browser",
            "User Data",
        ),
    ]

    profile_pattern = re.compile(
        r"^(Default|Profile \d+|Guest Profile)$",
        re.IGNORECASE,
    )

    for base in bases:
        if not os.path.isdir(base):
            continue

        try:
            for entry in os.scandir(base):
                if (
                    entry.is_dir(follow_symlinks=False)
                    and not is_reparse_point(entry.path)
                    and profile_pattern.match(entry.name)
                ):
                    result.extend([
                        os.path.join(entry.path, "Cache"),
                        os.path.join(entry.path, "Code Cache"),
                        os.path.join(entry.path, "GPUCache"),
                        os.path.join(
                            entry.path,
                            "Service Worker",
                            "CacheStorage",
                        ),
                    ])
        except OSError:
            pass

    firefox = os.path.join(
        user,
        "AppData",
        "Local",
        "Mozilla",
        "Firefox",
        "Profiles",
    )

    if os.path.isdir(firefox):
        try:
            for entry in os.scandir(firefox):
                if (
                    entry.is_dir(follow_symlinks=False)
                    and not is_reparse_point(entry.path)
                ):
                    result.extend([
                        os.path.join(entry.path, "cache2"),
                        os.path.join(entry.path, "startupCache"),
                    ])
        except OSError:
            pass

    result.append(
        os.path.join(
            user,
            "AppData",
            "Local",
            "Microsoft",
            "Windows",
            "INetCache",
        )
    )

    return normalize_paths(result)


def game_cache_roots():
    user = get_user_profile()

    return normalize_paths([
        os.path.join(
            user,
            "AppData",
            "Local",
            "D3DSCache",
        ),
        os.path.join(
            user,
            "AppData",
            "Local",
            "NVIDIA",
            "DXCache",
        ),
        os.path.join(
            user,
            "AppData",
            "Local",
            "NVIDIA",
            "GLCache",
        ),
        os.path.join(
            user,
            "AppData",
            "Roaming",
            "discord",
            "Cache",
        ),
        os.path.join(
            user,
            "AppData",
            "Roaming",
            "discord",
            "Code Cache",
        ),
        os.path.join(
            user,
            "AppData",
            "Roaming",
            "discord",
            "GPUCache",
        ),
        os.path.join(
            user,
            "AppData",
            "Local",
            "EpicGamesLauncher",
            "Saved",
            "webcache",
        ),
        os.path.join(
            user,
            "AppData",
            "Local",
            "Battle.net",
            "Cache",
        ),
    ])


def category_roots(category):
    mapping = {
        "temp": temp_roots,
        "thumbnails": thumbnail_roots,
        "privacy": browser_cache_roots,
        "games": game_cache_roots,
        "recent": recent_roots,
    }

    function = mapping.get(category)
    return function() if function else []


def category_accepts(category, path):
    if category != "thumbnails":
        return True

    name = os.path.basename(path).lower()

    return (
        name.endswith(".db")
        and (
            name.startswith("thumbcache")
            or name.startswith("iconcache")
        )
    )


# ============================================================
# Предварительный просмотр и удаление
# ============================================================

def scan_cleanup(categories, min_age_hours, token, progress):
    candidates = {}
    cutoff = time.time() - max(0, min_age_hours) * 3600
    scanned = 0

    for category in categories:
        if token.cancelled():
            break

        progress(
            f"Сканирование: "
            f"{CATEGORY_NAMES.get(category, category)}"
        )

        for root in category_roots(category):
            if token.cancelled():
                break

            if (
                not os.path.isdir(root)
                or is_reparse_point(root)
            ):
                continue

            stack = [root]

            while stack and not token.cancelled():
                current = stack.pop()

                if (
                    current != root
                    and is_reparse_point(current)
                ):
                    continue

                try:
                    with os.scandir(current) as iterator:
                        for entry in iterator:
                            if token.cancelled():
                                break

                            try:
                                if (
                                    entry.is_symlink()
                                    or is_reparse_point(entry.path)
                                ):
                                    continue

                                if entry.is_dir(
                                    follow_symlinks=False
                                ):
                                    stack.append(entry.path)
                                    continue

                                if not entry.is_file(
                                    follow_symlinks=False
                                ):
                                    continue

                                scanned += 1

                                if scanned % 300 == 0:
                                    progress(
                                        f"Проверено файлов: {scanned}"
                                    )

                                if not category_accepts(
                                    category,
                                    entry.path,
                                ):
                                    continue

                                info = entry.stat(
                                    follow_symlinks=False
                                )

                                if (
                                    min_age_hours > 0
                                    and info.st_mtime > cutoff
                                ):
                                    continue

                                valid, _ = validate_delete_path(
                                    entry.path,
                                    root,
                                )

                                if not valid:
                                    continue

                                candidate = Candidate(
                                    category=category,
                                    path=entry.path,
                                    root=root,
                                    size=int(info.st_size),
                                    modified=float(info.st_mtime),
                                )

                                candidates[
                                    os.path.normcase(entry.path)
                                ] = candidate

                            except (
                                OSError,
                                PermissionError,
                                FileNotFoundError,
                            ):
                                continue

                except (
                    OSError,
                    PermissionError,
                    FileNotFoundError,
                ):
                    continue

    result = list(candidates.values())
    result.sort(key=lambda item: item.size, reverse=True)
    return result


def delete_candidates(candidates, token, progress):
    result = OperationResult()
    total = len(candidates)

    for index, candidate in enumerate(candidates, 1):
        if token.cancelled():
            result.status = "cancelled"
            result.cancelled = True
            result.message = "Отменено пользователем"
            break

        if index % 20 == 0 or index == total:
            progress(f"Удаление: {index}/{total}")

        valid, reason = validate_delete_path(
            candidate.path,
            candidate.root,
        )

        if not valid:
            result.skipped += 1
            result.details.append({
                "path": candidate.path,
                "success": False,
                "error": reason,
            })
            continue

        try:
            if not os.path.isfile(candidate.path):
                result.skipped += 1
                continue

            size = os.path.getsize(candidate.path)
            os.remove(candidate.path)

            result.deleted += 1
            result.bytes_freed += size
            result.details.append({
                "path": candidate.path,
                "success": True,
                "bytes": size,
            })

        except FileNotFoundError:
            result.skipped += 1

        except PermissionError as error:
            result.access_denied += 1
            result.details.append({
                "path": candidate.path,
                "success": False,
                "error": str(error),
            })

        except OSError as error:
            if getattr(error, "winerror", None) in (32, 33):
                result.locked += 1
            else:
                result.errors += 1

            result.details.append({
                "path": candidate.path,
                "success": False,
                "error": str(error),
            })

    if result.status != "cancelled":
        if result.errors or result.locked or result.access_denied:
            result.status = "partial"
        else:
            result.status = "success"

    return result


# ============================================================
# Корзина и DNS
# ============================================================

class SHQUERYRBINFO(ctypes.Structure):
    _fields_ = [
        ("cbSize", ctypes.c_ulong),
        ("i64Size", ctypes.c_longlong),
        ("i64NumItems", ctypes.c_longlong),
    ]


def drive_letters():
    result = []

    try:
        mask = ctypes.windll.kernel32.GetLogicalDrives()

        for letter in string.ascii_uppercase:
            if mask & 1:
                path = f"{letter}:\\"

                if os.path.exists(path):
                    result.append(path)

            mask >>= 1

    except Exception:
        logging.exception("Ошибка получения дисков")

    return result


def recycle_size():
    total = 0

    for drive in drive_letters():
        try:
            info = SHQUERYRBINFO()
            info.cbSize = ctypes.sizeof(SHQUERYRBINFO)

            code = ctypes.windll.shell32.SHQueryRecycleBinW(
                ctypes.c_wchar_p(drive),
                ctypes.byref(info),
            )

            if code == 0:
                total += int(info.i64Size)

        except Exception:
            continue

    return total


def empty_recycle_bin():
    result = OperationResult()
    expected = recycle_size()

    try:
        flags = 0x1 | 0x2 | 0x4

        code = ctypes.windll.shell32.SHEmptyRecycleBinW(
            None,
            None,
            flags,
        )

        if code == 0:
            result.deleted = 1 if expected else 0
            result.bytes_freed = expected
            result.message = "Корзина очищена"
        else:
            result.status = "failed"
            result.errors = 1
            result.message = f"Код WinAPI: {code}"

    except Exception as error:
        result.status = "failed"
        result.errors = 1
        result.message = str(error)

    return result


def flush_dns():
    result = OperationResult()
    command = run_command(["ipconfig", "/flushdns"], 30)

    if command["success"]:
        result.deleted = 1
        result.message = "Кэш DNS очищен"
    else:
        result.status = "failed"
        result.errors = 1
        result.message = (
            command["stderr"]
            or command["stdout"]
            or "Команда завершилась ошибкой"
        )

    return result


# ============================================================
# Обход файлов
# ============================================================

def iter_files(roots, token, progress):
    scanned = 0
    ignored = {
        "$recycle.bin",
        "system volume information",
        "windowsapps",
    }

    for root in normalize_paths(roots):
        if token.cancelled():
            return

        if (
            not os.path.isdir(root)
            or is_reparse_point(root)
        ):
            continue

        stack = [root]

        while stack and not token.cancelled():
            current = stack.pop()

            try:
                with os.scandir(current) as iterator:
                    for entry in iterator:
                        if token.cancelled():
                            return

                        try:
                            if entry.name.lower() in ignored:
                                continue

                            if (
                                entry.is_symlink()
                                or is_reparse_point(entry.path)
                            ):
                                continue

                            if entry.is_dir(
                                follow_symlinks=False
                            ):
                                stack.append(entry.path)

                            elif entry.is_file(
                                follow_symlinks=False
                            ):
                                scanned += 1

                                if scanned % 500 == 0:
                                    progress(
                                        f"Проверено файлов: {scanned}"
                                    )

                                yield entry.path

                        except OSError:
                            continue

            except OSError:
                continue


def scan_big_files(roots, min_mb, token, progress):
    threshold = int(min_mb) * 1024 * 1024
    result = []

    for path in iter_files(roots, token, progress):
        try:
            size = os.path.getsize(path)

            if size >= threshold:
                result.append({
                    "path": path,
                    "size": size,
                    "modified": os.path.getmtime(path),
                })

        except OSError:
            continue

    result.sort(key=lambda item: item["size"], reverse=True)
    return result[:1000]


def partial_hash(path, size):
    digest = hashlib.sha256()
    block = 1024 * 1024

    try:
        with open(path, "rb") as file:
            digest.update(file.read(block))

            if size > block * 2:
                file.seek(max(0, size - block))
                digest.update(file.read(block))

        digest.update(str(size).encode("ascii"))
        return digest.hexdigest()

    except OSError:
        return None


def full_hash(path, token):
    digest = hashlib.sha256()

    try:
        with open(path, "rb") as file:
            while not token.cancelled():
                chunk = file.read(2 * 1024 * 1024)

                if not chunk:
                    return digest.hexdigest()

                digest.update(chunk)

    except OSError:
        return None

    return None


def scan_duplicates(roots, min_mb, token, progress):
    threshold = int(min_mb) * 1024 * 1024
    sizes = {}

    for path in iter_files(roots, token, progress):
        try:
            size = os.path.getsize(path)

            if size >= threshold:
                sizes.setdefault(size, []).append(path)

        except OSError:
            continue

    partial_groups = {}
    checked = 0

    for size, paths in sizes.items():
        if token.cancelled():
            return []

        if len(paths) < 2:
            continue

        for path in paths:
            checked += 1

            if checked % 20 == 0:
                progress(
                    f"Быстрое сравнение: {checked}"
                )

            digest = partial_hash(path, size)

            if digest:
                partial_groups.setdefault(
                    (size, digest),
                    [],
                ).append(path)

    full_groups = {}
    checked = 0

    for (size, _), paths in partial_groups.items():
        if token.cancelled():
            return []

        if len(paths) < 2:
            continue

        for path in paths:
            checked += 1

            if checked % 10 == 0:
                progress(
                    f"Полное сравнение: {checked}"
                )

            digest = full_hash(path, token)

            if digest:
                full_groups.setdefault(
                    (size, digest),
                    [],
                ).append(path)

    result = []

    for (size, digest), paths in full_groups.items():
        if len(paths) >= 2:
            result.append({
                "size": size,
                "hash": digest,
                "files": paths,
                "wasted": size * (len(paths) - 1),
            })

    result.sort(
        key=lambda item: item["wasted"],
        reverse=True,
    )

    return result[:500]


# ============================================================
# Состояние системы
# ============================================================

def list_processes():
    if psutil is None:
        return []

    result = []

    for process in psutil.process_iter([
        "pid",
        "name",
        "username",
        "memory_info",
        "status",
    ]):
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

    result.sort(
        key=lambda item: item["memory"],
        reverse=True,
    )

    return result


def terminate_process(pid):
    if psutil is None:
        return False, "psutil не установлен"

    try:
        pid = int(pid)

        if pid in (0, 4, os.getpid()):
            return False, "Этот процесс завершать запрещено"

        process = psutil.Process(pid)
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

    except Exception as error:
        return False, str(error)


def system_snapshot():
    result = {
        "score": 50,
        "state": "Недоступно",
        "cpu": None,
        "memory": None,
        "disk_free": None,
        "disk_total": None,
        "warnings": [],
    }

    if psutil is None:
        result["warnings"].append("Установите psutil")
        return result

    try:
        cpu = psutil.cpu_percent(interval=0.15)
        memory = psutil.virtual_memory()
        disk = psutil.disk_usage(get_system_drive())
        score = 100
        warnings = []

        free_percent = (
            disk.free * 100 / disk.total
            if disk.total else 0
        )

        if free_percent < 10:
            score -= 30
            warnings.append(
                "На системном диске менее 10% свободного места"
            )
        elif free_percent < 20:
            score -= 15
            warnings.append(
                "На системном диске менее 20% свободного места"
            )

        if memory.percent > 90:
            score -= 20
            warnings.append("Очень высокая загрузка памяти")
        elif memory.percent > 75:
            score -= 10
            warnings.append("Высокая загрузка памяти")

        if cpu > 90:
            score -= 15
            warnings.append("Очень высокая загрузка CPU")

        if score >= 85:
            state = "Отлично"
        elif score >= 65:
            state = "Нормально"
        elif score >= 40:
            state = "Есть замечания"
        else:
            state = "Требует внимания"

        result.update({
            "score": max(0, score),
            "state": state,
            "cpu": cpu,
            "memory": memory.percent,
            "disk_free": disk.free,
            "disk_total": disk.total,
            "warnings": warnings,
        })

    except Exception as error:
        result["warnings"].append(str(error))

    return result


# ============================================================
# Автозагрузка
# ============================================================

def startup_folders():
    program_data = os.environ.get(
        "ProgramData",
        r"C:\ProgramData",
    )

    return normalize_paths([
        os.path.join(
            get_user_profile(),
            "AppData",
            "Roaming",
            "Microsoft",
            "Windows",
            "Start Menu",
            "Programs",
            "Startup",
        ),
        os.path.join(
            program_data,
            "Microsoft",
            "Windows",
            "Start Menu",
            "Programs",
            "Startup",
        ),
    ])


def startup_views():
    if winreg is None:
        return []

    result = [("HKCU", 0)]

    view64 = getattr(winreg, "KEY_WOW64_64KEY", 0)
    view32 = getattr(winreg, "KEY_WOW64_32KEY", 0)

    if sys_is_64_bit():
        if view64:
            result.append(("HKLM", view64))
        if view32:
            result.append(("HKLM", view32))
    else:
        result.append(("HKLM", 0))

    return result


def sys_is_64_bit():
    import sys
    return sys.maxsize > 2**32


def registry_root(name):
    if winreg is None:
        return None

    return (
        winreg.HKEY_CURRENT_USER
        if name == "HKCU"
        else winreg.HKEY_LOCAL_MACHINE
    )


def list_startup_items():
    result = []

    if winreg is not None:
        for root_name, view in startup_views():
            try:
                with winreg.OpenKey(
                    registry_root(root_name),
                    STARTUP_RUN_PATH,
                    0,
                    winreg.KEY_READ | view,
                ) as key:
                    index = 0

                    while True:
                        try:
                            name, value, reg_type = (
                                winreg.EnumValue(key, index)
                            )
                            index += 1

                            result.append({
                                "kind": "registry",
                                "name": name,
                                "value": value,
                                "reg_type": reg_type,
                                "root": root_name,
                                "view": view,
                                "source": (
                                    f"{root_name}\\"
                                    f"{STARTUP_RUN_PATH}"
                                ),
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
                        "kind": "file",
                        "name": entry.name,
                        "value": entry.path,
                        "path": entry.path,
                        "source": folder,
                    })
        except OSError:
            continue

    return result


def encode_registry_value(value, reg_type):
    if isinstance(value, bytes):
        value = base64.b64encode(value).decode("ascii")
        encoding = "base64"
    else:
        encoding = "json"

    return {
        "encoding": encoding,
        "value": value,
        "type": reg_type,
    }


def decode_registry_value(data):
    value = data.get("value")

    if data.get("encoding") == "base64":
        value = base64.b64decode(value)

    return value, int(data.get("type", 1))


def disable_startup(item):
    backups = load_json(STARTUP_BACKUP_FILE, [])

    if not isinstance(backups, list):
        backups = []

    backup_id = datetime.now().strftime(
        "%Y%m%d_%H%M%S_%f"
    )

    if item.get("kind") == "registry":
        try:
            root_name = item["root"]
            view = int(item.get("view", 0))

            backup = {
                "id": backup_id,
                "kind": "registry",
                "name": item["name"],
                "root": root_name,
                "view": view,
                "source": item["source"],
                "created": datetime.now().isoformat(
                    timespec="seconds"
                ),
                "data": encode_registry_value(
                    item["value"],
                    item["reg_type"],
                ),
            }

            with winreg.OpenKey(
                registry_root(root_name),
                STARTUP_RUN_PATH,
                0,
                winreg.KEY_SET_VALUE | view,
            ) as key:
                winreg.DeleteValue(key, item["name"])

            backups.append(backup)
            save_json(STARTUP_BACKUP_FILE, backups)
            return True, "Запись отключена"

        except Exception as error:
            logging.exception("Ошибка отключения автозагрузки")
            return False, str(error)

    if item.get("kind") == "file":
        source = normalize_path(item.get("path"))

        if not os.path.isfile(source):
            return False, "Файл не найден"

        target = os.path.join(
            STARTUP_BACKUP_DIR,
            f"{backup_id}_{safe_filename(os.path.basename(source))}",
        )

        try:
            shutil.move(source, target)

            backups.append({
                "id": backup_id,
                "kind": "file",
                "name": item["name"],
                "original_path": source,
                "backup_path": target,
                "created": datetime.now().isoformat(
                    timespec="seconds"
                ),
            })

            save_json(STARTUP_BACKUP_FILE, backups)
            return True, "Файл отключён"

        except Exception as error:
            return False, str(error)

    return False, "Неизвестный тип элемента"


def restore_startup(backup_id):
    backups = load_json(STARTUP_BACKUP_FILE, [])

    backup = next(
        (
            item for item in backups
            if item.get("id") == backup_id
        ),
        None,
    )

    if not backup:
        return False, "Резервная копия не найдена"

    try:
        if backup["kind"] == "registry":
            value, reg_type = decode_registry_value(
                backup["data"]
            )

            with winreg.CreateKeyEx(
                registry_root(backup["root"]),
                STARTUP_RUN_PATH,
                0,
                winreg.KEY_SET_VALUE
                | int(backup.get("view", 0)),
            ) as key:
                winreg.SetValueEx(
                    key,
                    backup["name"],
                    0,
                    reg_type,
                    value,
                )

        elif backup["kind"] == "file":
            source = backup["backup_path"]
            target = backup["original_path"]

            if os.path.exists(target):
                return False, "Исходный файл уже существует"

            os.makedirs(
                os.path.dirname(target),
                exist_ok=True,
            )
            shutil.move(source, target)

        backups = [
            item for item in backups
            if item.get("id") != backup_id
        ]
        save_json(STARTUP_BACKUP_FILE, backups)

        return True, "Элемент восстановлен"

    except Exception as error:
        return False, str(error)


# ============================================================
# Отчёты
# ============================================================

def save_result_report(name, result):
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    path = os.path.join(
        REPORT_DIR,
        f"{safe_filename(name)}_{timestamp}.json",
    )

    data = {
        "application": APP_NAME,
        "version": APP_VERSION,
        "time": datetime.now().isoformat(
            timespec="seconds"
        ),
        "result": result.__dict__,
    }

    save_json(path, data)
    return path


def export_candidates_csv(candidates, path):
    try:
        with open(
            path,
            "w",
            encoding="utf-8-sig",
            newline="",
        ) as file:
            writer = csv.writer(file, delimiter=";")
            writer.writerow([
                "Категория",
                "Путь",
                "Размер",
                "Изменён",
            ])

            for candidate in candidates:
                writer.writerow([
                    CATEGORY_NAMES.get(
                        candidate.category,
                        candidate.category,
                    ),
                    candidate.path,
                    candidate.size,
                    datetime.fromtimestamp(
                        candidate.modified
                    ).isoformat(timespec="seconds"),
                ])

        return True, ""

    except Exception as error:
        return False, str(error)
