' Detached cargo verify — double-click or: wscript //B thisfile
Option Explicit
Dim sh, fso, out, cmd, rc
Set sh = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")
out = "D:\Project\Marionette\fortest\cargo-out.txt"
cmd = "cmd /c ""D:\Project\Marionette\fortest\run-all-cargo.bat"""
rc = sh.Run(cmd, 0, True)
' bat already writes cargo-out.txt
WScript.Quit rc
