# -*- coding: utf-8 -*-

import ctypes
import logging
import os
import sys

from PyQt5.QtWidgets import QApplication, QMessageBox

from core import (
    APP_NAME,
    APP_VERSION,
    SingleInstance,
)
from ui import MainWindow


def show_native_message(title, message, error=False):
    if os.name != "nt":
        try:
            print(f"{title}: {message}")
        except Exception:
            pass
        return

    try:
        flags = 0x10 if error else 0x40

        ctypes.windll.user32.MessageBoxW(
            None,
            str(message),
            str(title),
            flags,
        )
    except Exception:
        try:
            print(f"{title}: {message}")
        except Exception:
            pass


def main():
    if os.name != "nt":
        print(
            f"{APP_NAME} поддерживает только Windows."
        )
        return 1

    instance = SingleInstance()

    if instance.already_exists:
        show_native_message(
            APP_NAME,
            "Программа уже запущена.",
        )
        instance.close()
        return 1

    app = None

    try:
        logging.info(
            "%s %s запускается",
            APP_NAME,
            APP_VERSION,
        )

        app = QApplication(sys.argv)
        app.setApplicationName(APP_NAME)
        app.setApplicationVersion(APP_VERSION)
        app.setOrganizationName(APP_NAME)
        app.setStyle("Fusion")

        window = MainWindow()
        window.show()

        return app.exec_()

    except Exception as error:
        logging.exception(
            "Критическая ошибка приложения"
        )

        if app is not None:
            QMessageBox.critical(
                None,
                "Критическая ошибка",
                f"Приложение завершилось с ошибкой:\n\n"
                f"{error}",
            )
        else:
            show_native_message(
                "Критическая ошибка",
                str(error),
                error=True,
            )

        return 1

    finally:
        instance.close()
        logging.info(
            "%s завершён",
            APP_NAME,
        )


if __name__ == "__main__":
    sys.exit(main())
