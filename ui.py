# -*- coding: utf-8 -*-

import os
import re
from datetime import datetime

from PyQt5.QtCore import Qt, QTimer
from PyQt5.QtGui import QColor, QPalette
from PyQt5.QtWidgets import (
    QAbstractItemView,
    QApplication,
    QCheckBox,
    QFileDialog,
    QGridLayout,
    QGroupBox,
    QHBoxLayout,
    QHeaderView,
    QInputDialog,
    QLabel,
    QLineEdit,
    QMainWindow,
    QMessageBox,
    QProgressBar,
    QPushButton,
    QSpinBox,
    QStatusBar,
    QTabWidget,
    QTreeWidget,
    QTreeWidgetItem,
    QVBoxLayout,
    QWidget,
)

from core import (
    APP_NAME,
    APP_VERSION,
    LOG_DIR,
    REPORT_DIR,
    STARTUP_BACKUP_FILE,
    format_size,
    get_desktop,
    get_user_profile,
    load_json,
    load_settings,
    normalize_path,
    normalize_paths,
    save_settings,
)

from operations import (
    CATEGORY_NAMES,
    disable_startup,
    export_candidates_csv,
    recycle_size,
    restore_startup,
    save_result_report,
    terminate_process,
)

from workers import WorkerThread


STYLE = """
QMainWindow, QWidget {
    background: #0f1115;
    color: #e8e8e8;
    font-size: 13px;
}
QTabWidget::pane {
    border: 1px solid #242a35;
    background: #0f1115;
}
QTabBar::tab {
    background: #171a21;
    color: #bfc7d5;
    padding: 9px 13px;
    border: 1px solid #242a35;
    margin-right: 2px;
}
QTabBar::tab:selected {
    background: #1f6feb;
    color: white;
}
QGroupBox {
    border: 1px solid #2b3240;
    border-radius: 9px;
    margin-top: 12px;
    padding-top: 12px;
    font-weight: bold;
}
QGroupBox::title {
    subcontrol-origin: margin;
    left: 12px;
    padding: 0 5px;
}
QPushButton {
    background: #1f6feb;
    color: white;
    border: none;
    border-radius: 8px;
    padding: 9px 14px;
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
    color: #9aa0aa;
}
QLineEdit, QSpinBox {
    background: #11151d;
    color: #e8e8e8;
    border: 1px solid #2b3240;
    border-radius: 7px;
    padding: 7px;
}
QTreeWidget {
    background: #11151d;
    color: #e8e8e8;
    border: 1px solid #2b3240;
    alternate-background-color: #151a23;
}
QHeaderView::section {
    background: #171b24;
    color: #d7dce7;
    border: none;
    border-right: 1px solid #2b3240;
    padding: 7px;
}
QProgressBar {
    background: #11151d;
    color: #e8e8e8;
    border: 1px solid #2b3240;
    border-radius: 7px;
    text-align: center;
    min-height: 18px;
}
QProgressBar::chunk {
    background: #1f6feb;
    border-radius: 6px;
}
QStatusBar {
    background: #101318;
    color: #cfd7e3;
    border-top: 1px solid #242a35;
}
"""


class MainWindow(QMainWindow):
    def __init__(self):
        super().__init__()

        self.settings = load_settings()
        self.worker = None
        self.preview_candidates = []

        self.setWindowTitle(
            f"{APP_NAME} {APP_VERSION} — безопасная очистка"
        )
        self.setMinimumSize(1100, 720)
        self.resize(1350, 820)

        self.setStyleSheet(STYLE)
        self.apply_palette()

        self.tabs = QTabWidget()
        self.setCentralWidget(self.tabs)

        self.status_bar = QStatusBar()
        self.setStatusBar(self.status_bar)
        self.status_bar.showMessage("Готово")

        self.init_dashboard_tab()
        self.init_cleanup_tab()
        self.init_files_tab()
        self.init_startup_tab()
        self.init_processes_tab()
        self.init_settings_tab()

        self.tabs.currentChanged.connect(self.on_tab_changed)

        self.dashboard_timer = QTimer(self)
        self.dashboard_timer.timeout.connect(
            self.refresh_dashboard
        )
        self.dashboard_timer.start(10000)

        QTimer.singleShot(300, self.refresh_dashboard)

    # ========================================================
    # Общие методы интерфейса
    # ========================================================

    def apply_palette(self):
        palette = QPalette()
        palette.setColor(
            QPalette.Window,
            QColor(15, 17, 21),
        )
        palette.setColor(
            QPalette.WindowText,
            QColor(232, 232, 232),
        )
        palette.setColor(
            QPalette.Base,
            QColor(17, 21, 29),
        )
        palette.setColor(
            QPalette.AlternateBase,
            QColor(21, 26, 35),
        )
        palette.setColor(
            QPalette.Text,
            QColor(232, 232, 232),
        )
        palette.setColor(
            QPalette.Button,
            QColor(31, 111, 235),
        )
        palette.setColor(
            QPalette.ButtonText,
            QColor(255, 255, 255),
        )
        palette.setColor(
            QPalette.Highlight,
            QColor(31, 111, 235),
        )
        palette.setColor(
            QPalette.HighlightedText,
            QColor(255, 255, 255),
        )
        self.setPalette(palette)

    def make_title(self, title, subtitle=""):
        widget = QWidget()
        layout = QVBoxLayout(widget)
        layout.setContentsMargins(0, 0, 0, 8)
        layout.setSpacing(4)

        title_label = QLabel(title)
        title_label.setStyleSheet(
            "font-size: 24px; font-weight: 800; color: white;"
        )
        layout.addWidget(title_label)

        if subtitle:
            subtitle_label = QLabel(subtitle)
            subtitle_label.setWordWrap(True)
            subtitle_label.setStyleSheet(
                "color: #aab3c2;"
            )
            layout.addWidget(subtitle_label)

        return widget

    def make_metric(self, title):
        box = QGroupBox(title)
        layout = QVBoxLayout(box)

        label = QLabel("—")
        label.setStyleSheet(
            "font-size: 23px; font-weight: 800; color: white;"
        )

        layout.addWidget(label)
        return box, label

    def is_busy(self):
        return (
            self.worker is not None
            and self.worker.isRunning()
        )

    def start_worker(self, operation, payload=None):
        if self.is_busy():
            QMessageBox.information(
                self,
                "Операция выполняется",
                "Дождитесь завершения текущей операции "
                "или нажмите кнопку «Отмена».",
            )
            return False

        self.worker = WorkerThread(
            operation,
            payload or {},
            self,
        )

        self.worker.progress.connect(
            self.on_worker_progress
        )
        self.worker.completed.connect(
            self.on_worker_completed
        )
        self.worker.failed.connect(
            self.on_worker_failed
        )
        self.worker.finished.connect(
            self.on_worker_finished
        )

        self.operation_progress.setRange(0, 0)
        self.operation_label.setText("Операция запущена")
        self.button_cancel.setEnabled(True)
        self.worker.start()

        return True

    def cancel_worker(self):
        if not self.is_busy():
            return

        self.worker.cancel()
        self.status_bar.showMessage(
            "Запрошена отмена. Ожидайте завершения текущего шага..."
        )
        self.operation_label.setText(
            "Выполняется отмена..."
        )

    def on_worker_progress(self, message):
        self.status_bar.showMessage(message)
        self.operation_label.setText(message)

    def on_worker_finished(self):
        thread = self.worker
        self.worker = None

        self.operation_progress.setRange(0, 1)
        self.operation_progress.setValue(0)
        self.operation_label.setText("Готово")
        self.button_cancel.setEnabled(False)

        if thread is not None:
            thread.deleteLater()

    def on_worker_failed(self, operation, message):
        QMessageBox.critical(
            self,
            "Ошибка",
            f"Операция «{operation}» завершилась ошибкой:\n\n"
            f"{message}",
        )
        self.status_bar.showMessage(
            "Операция завершилась ошибкой"
        )

    def on_worker_completed(self, operation, data):
        if operation == "health":
            self.show_health(data)

        elif operation == "cleanup_scan":
            self.show_cleanup_preview(data)

        elif operation == "cleanup_delete":
            self.show_cleanup_result(data)

        elif operation == "big_files":
            self.show_big_files(data)

        elif operation == "duplicates":
            self.show_duplicates(data)

        elif operation == "processes":
            self.show_processes(data)

        elif operation == "startup":
            self.show_startup_items(data)

        elif operation == "recycle":
            self.show_special_result(
                "Очистка корзины",
                data,
            )

        elif operation == "dns":
            self.show_special_result(
                "Очистка DNS",
                data,
            )

        self.status_bar.showMessage("Готово")

    def open_path(self, path):
        path = normalize_path(path)

        if not path:
            return

        try:
            if os.path.isfile(path):
                os.startfile(
                    os.path.dirname(path)
                )
            elif os.path.isdir(path):
                os.startfile(path)
        except Exception as error:
            QMessageBox.warning(
                self,
                "Открытие папки",
                str(error),
            )

    # ========================================================
    # Dashboard
    # ========================================================

    def init_dashboard_tab(self):
        tab = QWidget()
        layout = QVBoxLayout(tab)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(12)

        layout.addWidget(
            self.make_title(
                APP_NAME,
                "Безопасная очистка и анализ Windows",
            )
        )

        grid = QGridLayout()

        self.health_box, self.health_label = (
            self.make_metric("Здоровье системы")
        )
        self.disk_box, self.disk_label = (
            self.make_metric("Свободно на диске")
        )
        self.memory_box, self.memory_label = (
            self.make_metric("Оперативная память")
        )
        self.cpu_box, self.cpu_label = (
            self.make_metric("CPU")
        )
        self.cleanup_box, self.last_cleanup_label = (
            self.make_metric("Последняя очистка")
        )
        self.freed_box, self.last_freed_label = (
            self.make_metric("Освобождено")
        )

        grid.addWidget(self.health_box, 0, 0)
        grid.addWidget(self.disk_box, 0, 1)
        grid.addWidget(self.memory_box, 0, 2)
        grid.addWidget(self.cpu_box, 1, 0)
        grid.addWidget(self.cleanup_box, 1, 1)
        grid.addWidget(self.freed_box, 1, 2)

        layout.addLayout(grid)

        self.health_progress = QProgressBar()
        self.health_progress.setRange(0, 100)
        self.health_progress.setValue(0)
        self.health_progress.setFormat(
            "Здоровье системы: %p%"
        )
        layout.addWidget(self.health_progress)

        self.warning_tree = QTreeWidget()
        self.warning_tree.setHeaderLabels(
            ["Предупреждения"]
        )
        self.warning_tree.header().setSectionResizeMode(
            0,
            QHeaderView.Stretch,
        )
        layout.addWidget(self.warning_tree)

        buttons = QHBoxLayout()

        refresh_button = QPushButton("Обновить")
        refresh_button.clicked.connect(
            self.refresh_dashboard
        )

        cleanup_button = QPushButton(
            "Перейти к очистке"
        )
        cleanup_button.clicked.connect(
            lambda: self.tabs.setCurrentWidget(
                self.cleanup_tab
            )
        )

        buttons.addWidget(refresh_button)
        buttons.addWidget(cleanup_button)
        buttons.addStretch()

        layout.addLayout(buttons)

        operation_box = QGroupBox(
            "Текущая фоновая операция"
        )
        operation_layout = QVBoxLayout(operation_box)

        self.operation_label = QLabel(
            "Нет активных операций"
        )

        self.operation_progress = QProgressBar()
        self.operation_progress.setRange(0, 1)
        self.operation_progress.setValue(0)

        self.button_cancel = QPushButton("Отмена")
        self.button_cancel.setEnabled(False)
        self.button_cancel.clicked.connect(
            self.cancel_worker
        )

        operation_layout.addWidget(
            self.operation_label
        )
        operation_layout.addWidget(
            self.operation_progress
        )
        operation_layout.addWidget(
            self.button_cancel
        )

        layout.addWidget(operation_box)
        self.tabs.addTab(tab, "Dashboard")

    def refresh_dashboard(self):
        if not self.is_busy():
            self.start_worker("health")

    def show_health(self, data):
        score = int(data.get("score", 0))
        state = data.get("state", "—")

        self.health_label.setText(
            f"{score}/100 — {state}"
        )
        self.health_progress.setValue(score)

        disk_free = data.get("disk_free")
        disk_total = data.get("disk_total")

        if disk_free is not None:
            self.disk_label.setText(
                format_size(disk_free)
            )
            self.disk_box.setToolTip(
                f"Всего: {format_size(disk_total)}"
            )
        else:
            self.disk_label.setText("N/A")

        memory = data.get("memory")
        self.memory_label.setText(
            f"{memory:.0f}%"
            if memory is not None
            else "N/A"
        )

        cpu = data.get("cpu")
        self.cpu_label.setText(
            f"{cpu:.0f}%"
            if cpu is not None
            else "N/A"
        )

        last_cleanup = (
            self.settings.get("last_cleanup")
            or "Никогда"
        )
        self.last_cleanup_label.setText(
            last_cleanup
        )

        self.last_freed_label.setText(
            format_size(
                self.settings.get("last_freed", 0)
            )
        )

        self.warning_tree.clear()
        warnings = data.get("warnings") or []

        if warnings:
            for warning in warnings:
                self.warning_tree.addTopLevelItem(
                    QTreeWidgetItem([
                        f"⚠ {warning}"
                    ])
                )
        else:
            self.warning_tree.addTopLevelItem(
                QTreeWidgetItem([
                    "Серьёзных предупреждений не обнаружено"
                ])
            )

    # ========================================================
    # Очистка
    # ========================================================

    def init_cleanup_tab(self):
        self.cleanup_tab = QWidget()
        layout = QVBoxLayout(self.cleanup_tab)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(10)

        layout.addWidget(
            self.make_title(
                "Безопасная очистка",
                "Сначала программа показывает файлы. "
                "Удаляются только отмеченные элементы.",
            )
        )

        category_box = QGroupBox("Категории")
        category_layout = QGridLayout(category_box)

        self.cleanup_checks = {}

        categories = [
            ("temp", "Временные файлы"),
            ("thumbnails", "Кэш миниатюр"),
            ("privacy", "Кэши браузеров"),
            ("games", "Игровые кэши"),
            ("recent", "Недавние документы"),
        ]

        for index, (key, text) in enumerate(categories):
            checkbox = QCheckBox(text)
            checkbox.setChecked(True)
            self.cleanup_checks[key] = checkbox

            category_layout.addWidget(
                checkbox,
                index // 2,
                index % 2,
            )

        category_layout.addWidget(
            QLabel("Удалять файлы старше:"),
            3,
            0,
        )

        self.cleanup_age = QSpinBox()
        self.cleanup_age.setRange(0, 24 * 365)
        self.cleanup_age.setSuffix(" ч")
        self.cleanup_age.setValue(
            int(
                self.settings.get(
                    "min_age_hours",
                    72,
                )
            )
        )

        category_layout.addWidget(
            self.cleanup_age,
            3,
            1,
        )

        layout.addWidget(category_box)

        buttons = QHBoxLayout()

        self.scan_cleanup_button = QPushButton(
            "Найти файлы"
        )
        self.scan_cleanup_button.clicked.connect(
            self.request_cleanup_scan
        )

        self.delete_cleanup_button = QPushButton(
            "Удалить отмеченное"
        )
        self.delete_cleanup_button.setEnabled(False)
        self.delete_cleanup_button.clicked.connect(
            self.request_cleanup_delete
        )

        self.export_cleanup_button = QPushButton(
            "Экспорт CSV"
        )
        self.export_cleanup_button.setEnabled(False)
        self.export_cleanup_button.clicked.connect(
            self.export_cleanup
        )

        recycle_button = QPushButton(
            "Очистить корзину"
        )
        recycle_button.clicked.connect(
            self.request_recycle_cleanup
        )

        dns_button = QPushButton("Очистить DNS")
        dns_button.clicked.connect(
            lambda: self.start_worker("dns")
        )

        buttons.addWidget(self.scan_cleanup_button)
        buttons.addWidget(self.delete_cleanup_button)
        buttons.addWidget(self.export_cleanup_button)
        buttons.addWidget(recycle_button)
        buttons.addWidget(dns_button)
        buttons.addStretch()

        layout.addLayout(buttons)

        self.cleanup_summary = QLabel(
            "Предварительный просмотр не выполнен"
        )
        layout.addWidget(self.cleanup_summary)

        self.cleanup_tree = QTreeWidget()
        self.cleanup_tree.setAlternatingRowColors(True)
        self.cleanup_tree.setHeaderLabels([
            "Удалить",
            "Категория",
            "Путь",
            "Размер",
            "Изменён",
        ])

        self.cleanup_tree.header().setSectionResizeMode(
            0,
            QHeaderView.ResizeToContents,
        )
        self.cleanup_tree.header().setSectionResizeMode(
            1,
            QHeaderView.ResizeToContents,
        )
        self.cleanup_tree.header().setSectionResizeMode(
            2,
            QHeaderView.Stretch,
        )
        self.cleanup_tree.header().setSectionResizeMode(
            3,
            QHeaderView.ResizeToContents,
        )
        self.cleanup_tree.header().setSectionResizeMode(
            4,
            QHeaderView.ResizeToContents,
        )

        self.cleanup_tree.itemDoubleClicked.connect(
            self.open_cleanup_item
        )

        layout.addWidget(self.cleanup_tree)
        self.tabs.addTab(
            self.cleanup_tab,
            "Очистка",
        )

    def selected_cleanup_categories(self):
        return [
            key
            for key, checkbox
            in self.cleanup_checks.items()
            if checkbox.isChecked()
        ]

    def request_cleanup_scan(self):
        categories = self.selected_cleanup_categories()

        if not categories:
            QMessageBox.information(
                self,
                "Очистка",
                "Выберите хотя бы одну категорию.",
            )
            return

        self.cleanup_tree.clear()
        self.preview_candidates = []
        self.delete_cleanup_button.setEnabled(False)
        self.export_cleanup_button.setEnabled(False)

        self.start_worker(
            "cleanup_scan",
            {
                "categories": categories,
                "age": self.cleanup_age.value(),
            },
        )

    def show_cleanup_preview(self, candidates):
        self.preview_candidates = list(candidates)
        self.cleanup_tree.clear()

        total_size = 0

        for index, candidate in enumerate(
            self.preview_candidates
        ):
            total_size += candidate.size

            try:
                modified = datetime.fromtimestamp(
                    candidate.modified
                ).strftime("%Y-%m-%d %H:%M")
            except Exception:
                modified = "—"

            item = QTreeWidgetItem([
                "",
                CATEGORY_NAMES.get(
                    candidate.category,
                    candidate.category,
                ),
                candidate.path,
                format_size(candidate.size),
                modified,
            ])

            item.setFlags(
                item.flags()
                | Qt.ItemIsUserCheckable
            )
            item.setCheckState(0, Qt.Checked)
            item.setData(0, Qt.UserRole, index)
            item.setData(
                2,
                Qt.UserRole,
                candidate.path,
            )

            self.cleanup_tree.addTopLevelItem(item)

        self.cleanup_summary.setText(
            f"Найдено файлов: {len(candidates)}; "
            f"размер: {format_size(total_size)}"
        )

        has_items = bool(candidates)
        self.delete_cleanup_button.setEnabled(
            has_items
        )
        self.export_cleanup_button.setEnabled(
            has_items
        )

    def checked_cleanup_candidates(self):
        result = []

        for row in range(
            self.cleanup_tree.topLevelItemCount()
        ):
            item = self.cleanup_tree.topLevelItem(row)

            if item.checkState(0) != Qt.Checked:
                continue

            index = item.data(0, Qt.UserRole)

            if (
                isinstance(index, int)
                and 0 <= index < len(
                    self.preview_candidates
                )
            ):
                result.append(
                    self.preview_candidates[index]
                )

        return result

    def request_cleanup_delete(self):
        candidates = self.checked_cleanup_candidates()

        if not candidates:
            QMessageBox.information(
                self,
                "Очистка",
                "Нет отмеченных файлов.",
            )
            return

        total_size = sum(
            candidate.size
            for candidate in candidates
        )

        answer = QMessageBox.warning(
            self,
            "Подтверждение",
            f"Будет удалено файлов: {len(candidates)}\n"
            f"Размер: {format_size(total_size)}\n\n"
            "Файлы удаляются без помещения в корзину.\n"
            "Продолжить?",
            QMessageBox.Yes | QMessageBox.No,
            QMessageBox.No,
        )

        if answer != QMessageBox.Yes:
            return

        self.start_worker(
            "cleanup_delete",
            {"candidates": candidates},
        )

    def show_cleanup_result(self, result):
        report_path = save_result_report(
            "cleanup",
            result,
        )

        self.settings["last_cleanup"] = (
            datetime.now().strftime(
                "%Y-%m-%d %H:%M:%S"
            )
        )
        self.settings["last_freed"] = (
            result.bytes_freed
        )
        save_settings(self.settings)

        message = (
            f"Статус: {result.status}\n"
            f"Удалено: {result.deleted}\n"
            f"Освобождено: "
            f"{format_size(result.bytes_freed)}\n"
            f"Пропущено: {result.skipped}\n"
            f"Заблокировано: {result.locked}\n"
            f"Нет доступа: {result.access_denied}\n"
            f"Ошибок: {result.errors}\n\n"
            f"Отчёт:\n{report_path}"
        )

        if result.status == "success":
            QMessageBox.information(
                self,
                "Очистка завершена",
                message,
            )
        else:
            QMessageBox.warning(
                self,
                "Очистка завершена",
                message,
            )

        self.preview_candidates = []
        self.cleanup_tree.clear()
        self.delete_cleanup_button.setEnabled(False)
        self.export_cleanup_button.setEnabled(False)
        self.cleanup_summary.setText(
            "Выполните новый поиск для обновления списка."
        )

        self.health_label.setText("Обновление…")

    def export_cleanup(self):
        if not self.preview_candidates:
            return

        default_path = os.path.join(
            get_desktop(),
            "pc_optimizer_preview.csv",
        )

        path, _ = QFileDialog.getSaveFileName(
            self,
            "Экспорт списка",
            default_path,
            "CSV (*.csv)",
        )

        if not path:
            return

        if not path.lower().endswith(".csv"):
            path += ".csv"

        success, message = export_candidates_csv(
            self.preview_candidates,
            path,
        )

        if success:
            QMessageBox.information(
                self,
                "Экспорт",
                f"Файл сохранён:\n{path}",
            )
        else:
            QMessageBox.warning(
                self,
                "Ошибка экспорта",
                message,
            )

    def request_recycle_cleanup(self):
        size = recycle_size()

        answer = QMessageBox.question(
            self,
            "Очистка корзины",
            f"Приблизительный размер корзины: "
            f"{format_size(size)}\n\n"
            "Очистить корзину?",
            QMessageBox.Yes | QMessageBox.No,
        )

        if answer == QMessageBox.Yes:
            self.start_worker("recycle")

    def show_special_result(self, title, result):
        report_path = save_result_report(
            title,
            result,
        )

        message = (
            f"{result.message}\n\n"
            f"Статус: {result.status}\n"
            f"Освобождено: "
            f"{format_size(result.bytes_freed)}\n"
            f"Ошибок: {result.errors}\n\n"
            f"Отчёт:\n{report_path}"
        )

        if result.status == "success":
            QMessageBox.information(
                self,
                title,
                message,
            )
        else:
            QMessageBox.warning(
                self,
                title,
                message,
            )

    def open_cleanup_item(self, item, column):
        path = item.data(2, Qt.UserRole)

        if path:
            self.open_path(path)

    # ========================================================
    # Большие файлы и дубликаты
    # ========================================================

    def init_files_tab(self):
        tab = QWidget()
        layout = QVBoxLayout(tab)
        layout.setContentsMargins(20, 20, 20, 20)
        layout.setSpacing(10)

        layout.addWidget(
            self.make_title(
                "Анализ файлов",
                "Поиск больших файлов и дубликатов "
                "без автоматического удаления.",
            )
        )

        root_box = QGroupBox("Папки поиска")
        root_layout = QHBoxLayout(root_box)

        self.file_roots = QLineEdit(
            get_user_profile()
        )
        self.file_roots.setPlaceholderText(
            "Папки разделяются точкой с запятой"
        )

        add_button = QPushButton("Добавить папку")
        add_button.clicked.connect(
            self.add_file_root
        )

        root_layout.addWidget(self.file_roots, 1)
        root_layout.addWidget(add_button)

        layout.addWidget(root_box)

        options = QHBoxLayout()

        options.addWidget(QLabel("Большие файлы от:"))

        self.big_file_size = QSpinBox()
        self.big_file_size.setRange(1, 100000)
        self.big_file_size.setSuffix(" МБ")
        self.big_file_size.setValue(
            int(
                self.settings.get(
                    "big_file_mb",
                    250,
                )
            )
        )

        big_button = QPushButton(
            "Найти большие файлы"
        )
        big_button.clicked.connect(
            self.request_big_files
        )

        options.addWidget(self.big_file_size)
        options.addWidget(big_button)
        options.addSpacing(20)

        options.addWidget(QLabel("Дубликаты от:"))

        self.duplicate_size = QSpinBox()
        self.duplicate_size.setRange(1, 100000)
        self.duplicate_size.setSuffix(" МБ")
        self.duplicate_size.setValue(
            int(
                self.settings.get(
                    "duplicate_mb",
                    20,
                )
            )
        )

        duplicate_button = QPushButton(
            "Найти дубликаты"
        )
        duplicate_button.clicked.connect(
            self.request_duplicates
        )

        options.addWidget(self.duplicate_size)
        options.addWidget(duplicate_button)
        options.addStretch()

        layout.addLayout(options)

        self.file_tree = QTreeWidget()
        self.file_tree.setAlternatingRowColors(True)
        self.file_tree.setHeaderLabels([
            "Группа / Путь",
            "Размер",
            "Дополнительно",
        ])

        self.file_tree.header().setSectionResizeMode(
            0,
            QHeaderView.Stretch,
        )
        self.file_tree.header().setSectionResizeMode(
            1,
            QHeaderView.ResizeToContents,
        )
        self.file_tree.header().setSectionResizeMode(
            2,
            QHeaderView.ResizeToContents,
        )

        self.file_tree.itemDoubleClicked.connect(
            self.open_file_item
        )

        layout.addWidget(self.file_tree)
        self.tabs.addTab(tab, "Файлы")

    def selected_file_roots(self):
        values = re.split(
            r"[;\n]+",
            self.file_roots.text(),
        )

        return [
            path
            for path in normalize_paths(values)
            if os.path.isdir(path)
        ]

    def add_file_root(self):
        folder = QFileDialog.getExistingDirectory(
            self,
            "Выберите папку",
            get_user_profile(),
        )

        if not folder:
            return

        roots = self.selected_file_roots()

        if os.path.normcase(folder) not in {
            os.path.normcase(path)
            for path in roots
        }:
            roots.append(folder)

        self.file_roots.setText(
            "; ".join(roots)
        )

    def request_big_files(self):
        roots = self.selected_file_roots()

        if not roots:
            QMessageBox.information(
                self,
                "Анализ файлов",
                "Укажите существующую папку.",
            )
            return

        self.file_tree.clear()

        self.start_worker(
            "big_files",
            {
                "roots": roots,
                "min_mb": self.big_file_size.value(),
            },
        )

    def show_big_files(self, rows):
        self.file_tree.clear()

        for row in rows:
            try:
                modified = datetime.fromtimestamp(
                    row["modified"]
                ).strftime("%Y-%m-%d %H:%M")
            except Exception:
                modified = "—"

            item = QTreeWidgetItem([
                row["path"],
                format_size(row["size"]),
                modified,
            ])
            item.setData(
                0,
                Qt.UserRole,
                row["path"],
            )
            self.file_tree.addTopLevelItem(item)

        if not rows:
            self.file_tree.addTopLevelItem(
                QTreeWidgetItem([
                    "Подходящие файлы не найдены",
                    "",
                    "",
                ])
            )

    def request_duplicates(self):
        roots = self.selected_file_roots()

        if not roots:
            QMessageBox.information(
                self,
                "Дубликаты",
                "Укажите существующую папку.",
            )
            return

        self.file_tree.clear()

        self.start_worker(
            "duplicates",
            {
                "roots": roots,
                "min_mb": self.duplicate_size.value(),
            },
        )

    def show_duplicates(self, groups):
        self.file_tree.clear()

        for number, group in enumerate(groups, 1):
            parent = QTreeWidgetItem([
                f"Группа {number}: "
                f"{len(group['files'])} файлов",
                format_size(group["size"]),
                f"Лишние копии: "
                f"{format_size(group['wasted'])}",
            ])

            for path in group["files"]:
                child = QTreeWidgetItem([
                    path,
                    format_size(group["size"]),
                    group["hash"][:16] + "…",
                ])
                child.setData(
                    0,
                    Qt.UserRole,
                    path,
                )
                parent.addChild(child)

            parent.setExpanded(True)
            self.file_tree.addTopLevelItem(parent)

        if not groups:
            self.file_tree.addTopLevelItem(
                QTreeWidgetItem([
                    "Дубликаты не найдены",
                    "",
                    "",
                ])
            )

    def open_file_item(self, item, column):
        path = item.data(0, Qt.UserRole)

        if path:
            self.open_path(path)

    # ========================================================
    # Автозагрузка
    # ========================================================

    def init_startup_tab(self):
        tab = QWidget()
        layout = QVBoxLayout(tab)
        layout.setContentsMargins(20, 20, 20, 20)

        layout.addWidget(
            self.make_title(
                "Автозагрузка",
                "Отключённые элементы можно восстановить.",
            )
        )

        buttons = QHBoxLayout()

        refresh_button = QPushButton("Обновить")
        refresh_button.clicked.connect(
            self.refresh_startup
        )

        disable_button = QPushButton(
            "Отключить выбранное"
        )
        disable_button.clicked.connect(
            self.disable_selected_startup
        )

        restore_button = QPushButton(
            "Восстановить из резервной копии"
        )
        restore_button.clicked.connect(
            self.restore_startup_backup
        )

        buttons.addWidget(refresh_button)
        buttons.addWidget(disable_button)
        buttons.addWidget(restore_button)
        buttons.addStretch()

        layout.addLayout(buttons)

        self.startup_tree = QTreeWidget()
        self.startup_tree.setAlternatingRowColors(True)
        self.startup_tree.setSelectionMode(
            QAbstractItemView.SingleSelection
        )
        self.startup_tree.setHeaderLabels([
            "Тип",
            "Имя",
            "Источник",
            "Команда / Путь",
        ])

        self.startup_tree.header().setSectionResizeMode(
            0,
            QHeaderView.ResizeToContents,
        )
        self.startup_tree.header().setSectionResizeMode(
            1,
            QHeaderView.ResizeToContents,
        )
        self.startup_tree.header().setSectionResizeMode(
            2,
            QHeaderView.Stretch,
        )
        self.startup_tree.header().setSectionResizeMode(
            3,
            QHeaderView.Stretch,
        )

        layout.addWidget(self.startup_tree)
        self.tabs.addTab(tab, "Автозагрузка")

    def refresh_startup(self):
        if not self.is_busy():
            self.start_worker("startup")

    def show_startup_items(self, items):
        self.startup_tree.clear()

        for data in items:
            item = QTreeWidgetItem([
                (
                    "Реестр"
                    if data.get("kind") == "registry"
                    else "Файл"
                ),
                str(data.get("name", "")),
                str(data.get("source", "")),
                str(data.get("value", "")),
            ])
            item.setData(
                0,
                Qt.UserRole,
                data,
            )
            self.startup_tree.addTopLevelItem(item)

        if not items:
            self.startup_tree.addTopLevelItem(
                QTreeWidgetItem([
                    "—",
                    "Элементы не найдены",
                    "",
                    "",
                ])
            )

    def disable_selected_startup(self):
        item = self.startup_tree.currentItem()

        if item is None:
            QMessageBox.information(
                self,
                "Автозагрузка",
                "Выберите элемент.",
            )
            return

        data = item.data(0, Qt.UserRole)

        if not isinstance(data, dict):
            return

        answer = QMessageBox.question(
            self,
            "Автозагрузка",
            f"Отключить элемент?\n\n"
            f"{data.get('name', '')}\n"
            f"{data.get('value', '')}",
            QMessageBox.Yes | QMessageBox.No,
        )

        if answer != QMessageBox.Yes:
            return

        success, message = disable_startup(data)

        if success:
            QMessageBox.information(
                self,
                "Автозагрузка",
                message,
            )
            self.startup_tree.clear()
            QTimer.singleShot(
                100,
                self.refresh_startup,
            )
        else:
            QMessageBox.warning(
                self,
                "Ошибка",
                message,
            )

    def restore_startup_backup(self):
        backups = load_json(
            STARTUP_BACKUP_FILE,
            [],
        )

        if not isinstance(backups, list) or not backups:
            QMessageBox.information(
                self,
                "Резервные копии",
                "Резервные копии отсутствуют.",
            )
            return

        labels = []

        for backup in backups:
            labels.append(
                f"{backup.get('name', '—')} | "
                f"{backup.get('kind', '—')} | "
                f"{backup.get('created', '—')} | "
                f"{backup.get('id', '')}"
            )

        selected_label, accepted = (
            QInputDialog.getItem(
                self,
                "Восстановление автозагрузки",
                "Выберите элемент:",
                labels,
                0,
                False,
            )
        )

        if not accepted or not selected_label:
            return

        selected_index = labels.index(selected_label)
        selected = backups[selected_index]

        success, message = restore_startup(
            selected.get("id")
        )

        if success:
            QMessageBox.information(
                self,
                "Восстановление",
                message,
            )
            self.startup_tree.clear()
            QTimer.singleShot(
                100,
                self.refresh_startup,
            )
        else:
            QMessageBox.warning(
                self,
                "Ошибка восстановления",
                message,
            )

    # ========================================================
    # Процессы
    # ========================================================

    def init_processes_tab(self):
        tab = QWidget()
        layout = QVBoxLayout(tab)
        layout.setContentsMargins(20, 20, 20, 20)

        layout.addWidget(
            self.make_title(
                "Процессы",
                "Завершайте только известные процессы.",
            )
        )

        buttons = QHBoxLayout()

        refresh_button = QPushButton("Обновить")
        refresh_button.clicked.connect(
            self.refresh_processes
        )

        terminate_button = QPushButton(
            "Завершить выбранный"
        )
        terminate_button.clicked.connect(
            self.terminate_selected_process
        )

        buttons.addWidget(refresh_button)
        buttons.addWidget(terminate_button)
        buttons.addStretch()

        layout.addLayout(buttons)

        self.process_tree = QTreeWidget()
        self.process_tree.setAlternatingRowColors(True)
        self.process_tree.setHeaderLabels([
            "PID",
            "Имя",
            "Память",
            "Статус",
            "Пользователь",
        ])

        self.process_tree.header().setSectionResizeMode(
            0,
            QHeaderView.ResizeToContents,
        )
        self.process_tree.header().setSectionResizeMode(
            1,
            QHeaderView.Stretch,
        )
        self.process_tree.header().setSectionResizeMode(
            2,
            QHeaderView.ResizeToContents,
        )
        self.process_tree.header().setSectionResizeMode(
            3,
            QHeaderView.ResizeToContents,
        )
        self.process_tree.header().setSectionResizeMode(
            4,
            QHeaderView.Stretch,
        )

        layout.addWidget(self.process_tree)
        self.tabs.addTab(tab, "Процессы")

    def refresh_processes(self):
        if not self.is_busy():
            self.start_worker("processes")

    def show_processes(self, processes):
        self.process_tree.clear()

        for process in processes:
            item = QTreeWidgetItem([
                str(process.get("pid", 0)),
                process.get("name", ""),
                format_size(
                    process.get("memory", 0)
                ),
                process.get("status", ""),
                process.get("user", ""),
            ])

            item.setData(
                0,
                Qt.UserRole,
                process.get("pid"),
            )
            self.process_tree.addTopLevelItem(item)

    def terminate_selected_process(self):
        item = self.process_tree.currentItem()

        if item is None:
            QMessageBox.information(
                self,
                "Процессы",
                "Выберите процесс.",
            )
            return

        pid = item.data(0, Qt.UserRole)
        name = item.text(1)

        answer = QMessageBox.warning(
            self,
            "Завершение процесса",
            f"Завершить процесс?\n\n"
            f"{name} (PID {pid})\n\n"
            "Несохранённые данные могут быть потеряны.",
            QMessageBox.Yes | QMessageBox.No,
            QMessageBox.No,
        )

        if answer != QMessageBox.Yes:
            return

        success, message = terminate_process(pid)

        if success:
            QMessageBox.information(
                self,
                "Процессы",
                message,
            )
            self.process_tree.clear()
            QTimer.singleShot(
                100,
                self.refresh_processes,
            )
        else:
            QMessageBox.warning(
                self,
                "Ошибка",
                message,
            )

    # ========================================================
    # Настройки
    # ========================================================

    def init_settings_tab(self):
        tab = QWidget()
        layout = QVBoxLayout(tab)
        layout.setContentsMargins(20, 20, 20, 20)

        layout.addWidget(
            self.make_title(
                "Настройки",
                "Настройки применяются к очистке "
                "и анализу файлов.",
            )
        )

        box = QGroupBox("Основные параметры")
        grid = QGridLayout(box)

        self.setting_age = QSpinBox()
        self.setting_age.setRange(0, 24 * 365)
        self.setting_age.setSuffix(" ч")
        self.setting_age.setValue(
            int(
                self.settings.get(
                    "min_age_hours",
                    72,
                )
            )
        )

        self.setting_big = QSpinBox()
        self.setting_big.setRange(1, 100000)
        self.setting_big.setSuffix(" МБ")
        self.setting_big.setValue(
            int(
                self.settings.get(
                    "big_file_mb",
                    250,
                )
            )
        )

        self.setting_duplicate = QSpinBox()
        self.setting_duplicate.setRange(1, 100000)
        self.setting_duplicate.setSuffix(" МБ")
        self.setting_duplicate.setValue(
            int(
                self.settings.get(
                    "duplicate_mb",
                    20,
                )
            )
        )

        grid.addWidget(
            QLabel("Возраст файлов:"),
            0,
            0,
        )
        grid.addWidget(
            self.setting_age,
            0,
            1,
        )
        grid.addWidget(
            QLabel("Большие файлы от:"),
            1,
            0,
        )
        grid.addWidget(
            self.setting_big,
            1,
            1,
        )
        grid.addWidget(
            QLabel("Дубликаты от:"),
            2,
            0,
        )
        grid.addWidget(
            self.setting_duplicate,
            2,
            1,
        )

        layout.addWidget(box)

        buttons = QHBoxLayout()

        save_button = QPushButton(
            "Сохранить настройки"
        )
        save_button.clicked.connect(
            self.save_ui_settings
        )

        reports_button = QPushButton(
            "Открыть отчёты"
        )
        reports_button.clicked.connect(
            lambda: self.open_path(REPORT_DIR)
        )

        logs_button = QPushButton(
            "Открыть логи"
        )
        logs_button.clicked.connect(
            lambda: self.open_path(LOG_DIR)
        )

        buttons.addWidget(save_button)
        buttons.addWidget(reports_button)
        buttons.addWidget(logs_button)
        buttons.addStretch()

        layout.addLayout(buttons)

        information = QLabel(
            "Программа не удаляет Windows.old напрямую, "
            "не очищает весь каталог Windows\\Logs и "
            "не проходит через junction, symlink и другие "
            "reparse points."
        )
        information.setWordWrap(True)
        information.setStyleSheet(
            "color: #aab3c2; padding: 10px;"
        )

        layout.addWidget(information)
        layout.addStretch()

        self.tabs.addTab(tab, "Настройки")

    def save_ui_settings(self):
        self.settings["min_age_hours"] = (
            self.setting_age.value()
        )
        self.settings["big_file_mb"] = (
            self.setting_big.value()
        )
        self.settings["duplicate_mb"] = (
            self.setting_duplicate.value()
        )

        if save_settings(self.settings):
            self.cleanup_age.setValue(
                self.settings["min_age_hours"]
            )
            self.big_file_size.setValue(
                self.settings["big_file_mb"]
            )
            self.duplicate_size.setValue(
                self.settings["duplicate_mb"]
            )

            QMessageBox.information(
                self,
                "Настройки",
                "Настройки успешно сохранены.",
            )
        else:
            QMessageBox.warning(
                self,
                "Ошибка",
                "Не удалось сохранить настройки.",
            )

    # ========================================================
    # События окна
    # ========================================================

    def on_tab_changed(self, index):
        tab_name = self.tabs.tabText(index)

        if tab_name == "Dashboard":
            self.refresh_dashboard()

        elif tab_name == "Автозагрузка":
            if not self.is_busy():
                self.refresh_startup()

        elif tab_name == "Процессы":
            if not self.is_busy():
                self.refresh_processes()

    def closeEvent(self, event):
        if self.is_busy():
            answer = QMessageBox.question(
                self,
                "Операция выполняется",
                "Сейчас выполняется фоновая операция.\n\n"
                "Запросить её отмену?\n"
                "После отмены закройте программу ещё раз.",
                QMessageBox.Yes | QMessageBox.No,
                QMessageBox.Yes,
            )

            if answer == QMessageBox.Yes:
                self.cancel_worker()

            event.ignore()
            return

        self.dashboard_timer.stop()
        event.accept()
