# -*- coding: utf-8 -*-

import logging

from PyQt5.QtCore import QThread, pyqtSignal

from core import CancelToken
from operations import (
    delete_candidates,
    empty_recycle_bin,
    flush_dns,
    list_processes,
    list_startup_items,
    scan_big_files,
    scan_cleanup,
    scan_duplicates,
    system_snapshot,
)


class WorkerThread(QThread):
    progress = pyqtSignal(str)
    completed = pyqtSignal(str, object)
    failed = pyqtSignal(str, str)

    def __init__(self, operation, payload=None, parent=None):
        super().__init__(parent)
        self.operation = operation
        self.payload = payload or {}
        self.token = CancelToken()

    def cancel(self):
        self.token.cancel()
        self.requestInterruption()

    def report(self, message):
        self.progress.emit(str(message))

    def run(self):
        try:
            if self.operation == "health":
                data = system_snapshot()

            elif self.operation == "cleanup_scan":
                data = scan_cleanup(
                    self.payload.get("categories", []),
                    int(self.payload.get("age", 72)),
                    self.token,
                    self.report,
                )

            elif self.operation == "cleanup_delete":
                data = delete_candidates(
                    self.payload.get("candidates", []),
                    self.token,
                    self.report,
                )

            elif self.operation == "big_files":
                data = scan_big_files(
                    self.payload.get("roots", []),
                    int(self.payload.get("min_mb", 250)),
                    self.token,
                    self.report,
                )

            elif self.operation == "duplicates":
                data = scan_duplicates(
                    self.payload.get("roots", []),
                    int(self.payload.get("min_mb", 20)),
                    self.token,
                    self.report,
                )

            elif self.operation == "processes":
                data = list_processes()

            elif self.operation == "startup":
                data = list_startup_items()

            elif self.operation == "recycle":
                data = empty_recycle_bin()

            elif self.operation == "dns":
                data = flush_dns()

            else:
                raise RuntimeError(
                    f"Неизвестная операция: {self.operation}"
                )

            self.completed.emit(self.operation, data)

        except Exception as error:
            logging.exception(
                "Ошибка фоновой операции %s",
                self.operation,
            )
            self.failed.emit(self.operation, str(error))
