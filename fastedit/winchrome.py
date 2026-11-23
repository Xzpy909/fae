"""Frameless window on Windows: the UI's top bar is the title bar (unified header).

The window keeps a real resizable frame (WS_THICKFRAME | WS_CAPTION ...), so Windows treats it as a normal
window: Aero Snap, edge resize, Win+arrow keys, minimize / maximize animations, the drop shadow. Its
non-client area is collapsed to zero (WM_NCCALCSIZE, via a window subclass), so the page fills the whole
window and draws its own title bar. Because the page covers everything, Windows never sees the mouse on the
frame: the UI tells us when to start a move (caption) or resize (edge strip) and we hand it to Windows'
own loop (WM_NCLBUTTONDOWN + HTCAPTION / HTLEFT / ...).
Maximized: the window is fitted to the monitor's work area (taskbar stays visible); with an auto-hide
taskbar a 2 px gap is left on its edge so it can still slide in.
Everything touching the window runs on its UI thread (Invoke / BeginInvoke); JS API calls arrive on
another thread. Windows only; --native-frame keeps the normal Windows frame.
"""
from __future__ import annotations

import ctypes
import sys
import threading
from ctypes import wintypes

IS_WIN = sys.platform == "win32"

WM_NCLBUTTONDOWN, WM_NCCALCSIZE, WM_NCACTIVATE = 0xA1, 0x83, 0x86
HIT = {"caption": 2, "left": 10, "right": 11, "top": 12, "topleft": 13, "topright": 14,
       "bottom": 15, "bottomleft": 16, "bottomright": 17}
GWL_STYLE = -16
WS_CAPTION, WS_THICKFRAME, WS_SYSMENU = 0x00C00000, 0x00040000, 0x00080000
WS_MINIMIZEBOX, WS_MAXIMIZEBOX = 0x00020000, 0x00010000
SWP_FRAMECHANGED = 0x0020 | 0x0002 | 0x0001 | 0x0004 | 0x0010   # | NOMOVE | NOSIZE | NOZORDER | NOACTIVATE
SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE = 3, 6, 9
MONITOR_DEFAULTTONEAREST = 2
DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND = 33, 2
ABM_GETSTATE, ABM_GETAUTOHIDEBAREX, ABS_AUTOHIDE = 0x4, 0xB, 0x1
ABE_LEFT, ABE_TOP, ABE_RIGHT, ABE_BOTTOM = 0, 1, 2, 3

LRESULT = ctypes.c_ssize_t

# temporary diagnostics while the unified header is being tested: winchrome.log next to the settings file
import os as _os
import traceback as _tb
try:
    from .config import APP_DIR as _APP_DIR
except Exception:
    _APP_DIR = _os.getcwd()
_LOG = _os.path.join(_APP_DIR, "winchrome.log")


def _log(*a):
    try:
        with open(_LOG, "a", encoding="utf-8") as f:
            f.write(" ".join(str(x) for x in a) + "\n")
    except Exception:
        pass


class NCCALCSIZE_PARAMS(ctypes.Structure):
    _fields_ = [("rgrc", wintypes.RECT * 3), ("lppos", ctypes.c_void_p)]


class MONITORINFO(ctypes.Structure):
    _fields_ = [("cbSize", wintypes.DWORD), ("rcMonitor", wintypes.RECT), ("rcWork", wintypes.RECT),
                ("dwFlags", wintypes.DWORD)]


class APPBARDATA(ctypes.Structure):
    _fields_ = [("cbSize", wintypes.DWORD), ("hWnd", wintypes.HWND), ("uCallbackMessage", wintypes.UINT),
                ("uEdge", wintypes.UINT), ("rc", wintypes.RECT), ("lParam", ctypes.c_ssize_t)]


class MARGINS(ctypes.Structure):
    _fields_ = [("l", ctypes.c_int), ("r", ctypes.c_int), ("t", ctypes.c_int), ("b", ctypes.c_int)]


if IS_WIN:
    _u = ctypes.windll.user32
    _u.GetWindowLongPtrW.restype = ctypes.c_ssize_t
    _u.GetWindowLongPtrW.argtypes = [wintypes.HWND, ctypes.c_int]
    _u.SetWindowLongPtrW.restype = ctypes.c_ssize_t
    _u.SetWindowLongPtrW.argtypes = [wintypes.HWND, ctypes.c_int, ctypes.c_ssize_t]
    _u.SendMessageW.restype = LRESULT
    _u.SendMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    _u.MonitorFromWindow.restype = wintypes.HMONITOR
    _u.MonitorFromWindow.argtypes = [wintypes.HWND, wintypes.DWORD]
    _u.GetMonitorInfoW.argtypes = [wintypes.HMONITOR, ctypes.POINTER(MONITORINFO)]
    _u.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                                ctypes.c_int, wintypes.UINT]
    _u.IsZoomed.argtypes = [wintypes.HWND]
    _u.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
    _c = ctypes.windll.comctl32
    SUBCLASSPROC = ctypes.WINFUNCTYPE(LRESULT, wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM,
                                      ctypes.c_size_t, ctypes.c_size_t)
    _c.SetWindowSubclass.argtypes = [wintypes.HWND, SUBCLASSPROC, ctypes.c_size_t, ctypes.c_size_t]
    _c.DefSubclassProc.restype = LRESULT
    _c.DefSubclassProc.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    _s = ctypes.windll.shell32
    _s.SHAppBarMessage.restype = ctypes.c_size_t
    _s.SHAppBarMessage.argtypes = [wintypes.DWORD, ctypes.POINTER(APPBARDATA)]


def _work_area(hwnd):
    """The monitor's work area; with an auto-hide taskbar on that monitor, 2 px less on its edge."""
    mi = MONITORINFO(cbSize=ctypes.sizeof(MONITORINFO))
    _u.GetMonitorInfoW(_u.MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), ctypes.byref(mi))
    r = wintypes.RECT(mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom)
    abd = APPBARDATA(cbSize=ctypes.sizeof(APPBARDATA))
    if _s.SHAppBarMessage(ABM_GETSTATE, ctypes.byref(abd)) & ABS_AUTOHIDE:
        for edge in (ABE_BOTTOM, ABE_TOP, ABE_LEFT, ABE_RIGHT):
            q = APPBARDATA(cbSize=ctypes.sizeof(APPBARDATA), uEdge=edge, rc=mi.rcMonitor)
            if _s.SHAppBarMessage(ABM_GETAUTOHIDEBAREX, ctypes.byref(q)):
                if edge == ABE_BOTTOM:
                    r.bottom -= 2
                elif edge == ABE_TOP:
                    r.top += 2
                elif edge == ABE_LEFT:
                    r.left += 2
                else:
                    r.right -= 2
    return r


class Chrome:
    def __init__(self, window):
        self.w = window
        self._max = False
        self._proc = None            # the subclass callback must stay alive as long as the window

    # ---- UI-thread helpers ------------------------------------------------
    def _form(self):
        try:
            return self.w.native
        except Exception:
            return None

    def _hwnd(self):
        h = self._form().Handle
        return int(h.ToInt64()) if hasattr(h, "ToInt64") else int(h)     # IntPtr (pythonnet 3) or int

    def _ui(self, fn, wait=False):
        form = self._form()
        if form is None:
            _log("ui: no native form", type(self.w).__name__)
            return

        def safe():
            try:
                fn()
            except Exception:
                _log("ui error:", _tb.format_exc())
        from System import Action
        (form.Invoke if wait else form.BeginInvoke)(Action(safe))

    # ---- setup (once the window is shown) ---------------------------------
    def setup(self, *args):
        try:
            open(_LOG, "w").close()
        except Exception:
            pass
        _log("setup: called, native =", type(self._form()).__name__)

        def go():
            hwnd = self._hwnd()
            style = _u.GetWindowLongPtrW(hwnd, GWL_STYLE)
            _log("setup: hwnd", hex(hwnd), "style before", hex(style & 0xFFFFFFFF))
            _u.SetWindowLongPtrW(hwnd, GWL_STYLE, style | WS_CAPTION | WS_THICKFRAME | WS_SYSMENU
                                 | WS_MINIMIZEBOX | WS_MAXIMIZEBOX)
            _log("setup: style after", hex(_u.GetWindowLongPtrW(hwnd, GWL_STYLE) & 0xFFFFFFFF))

            def proc(h, msg, wp, lp, _id, _data):
                try:
                    if msg == WM_NCCALCSIZE and wp:
                        if _u.IsZoomed(h):             # maximized: fit the work area (not past the screen edges)
                            p = ctypes.cast(lp, ctypes.POINTER(NCCALCSIZE_PARAMS)).contents
                            p.rgrc[0] = _work_area(h)
                        return 0                       # no non-client area: the page fills the window
                    if msg == WM_NCACTIVATE:
                        return _c.DefSubclassProc(h, msg, wp, -1)     # don't repaint a (hidden) frame
                except Exception:
                    pass                               # never break the window: fall back to the default
                return _c.DefSubclassProc(h, msg, wp, lp)
            self._proc = SUBCLASSPROC(proc)
            _log("setup: subclass", _c.SetWindowSubclass(hwnd, self._proc, 1, 0))
            try:                                       # keeps the DWM shadow with a zero-size frame
                ctypes.windll.dwmapi.DwmExtendFrameIntoClientArea(wintypes.HWND(hwnd),
                                                                  ctypes.byref(MARGINS(0, 0, 1, 0)))
            except Exception:
                pass
            try:                                       # Windows 11 only; harmless elsewhere
                pref = ctypes.c_int(DWMWCP_ROUND)
                ctypes.windll.dwmapi.DwmSetWindowAttribute(wintypes.HWND(hwnd), DWMWA_WINDOW_CORNER_PREFERENCE,
                                                           ctypes.byref(pref), ctypes.sizeof(pref))
            except Exception:
                pass
            _log("setup: frame changed", _u.SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED))
            from System import EventHandler
            self._form().Resize += EventHandler(lambda s, e: self._state_changed())
            self._state_changed()
            _log("setup: done")
        self._ui(go)

    def _state_changed(self):
        """Tell the UI when the window is maximized / restored (the maximize button swaps its icon)."""
        try:
            m = bool(_u.IsZoomed(self._hwnd()))
        except Exception:
            return
        if m == self._max:
            return
        self._max = m
        js = f"window.app && window.app.winState && window.app.winState({'true' if m else 'false'})"
        threading.Thread(target=lambda: self.w.evaluate_js(js), daemon=True).start()   # never from the UI thread

    # ---- actions from the UI ------------------------------------------------
    def info(self):
        return {"frameless": True, "maximized": self._max}

    def begin(self, what):
        """Start Windows' own move (caption) or resize (edge) loop while the mouse button is down."""
        ht = HIT.get(what)
        if ht is None:
            return

        def go():
            hwnd = self._hwnd()
            _log("begin", what, "hwnd", hex(hwnd), "style", hex(_u.GetWindowLongPtrW(hwnd, GWL_STYLE) & 0xFFFFFFFF),
                 "zoomed", bool(_u.IsZoomed(hwnd)))
            _u.ReleaseCapture()
            _u.SendMessageW(hwnd, WM_NCLBUTTONDOWN, ht, 0)
            _log("begin", what, "loop ended")
        self._ui(go)

    def toggle_max(self):
        self._ui(lambda: _u.ShowWindow(self._hwnd(), SW_RESTORE if _u.IsZoomed(self._hwnd()) else SW_MAXIMIZE))

    def minimize(self):
        self._ui(lambda: _u.ShowWindow(self._hwnd(), SW_MINIMIZE))

    def close(self):
        self.w.destroy()
