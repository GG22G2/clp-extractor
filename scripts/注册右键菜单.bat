@echo off
chcp 65001 >nul
title CLP 鼠标右键菜单注册程序

echo ============================================================
echo         CLP 日志解压工具 - Windows 鼠标右键菜单注册
echo ============================================================
echo.

set "TOOL_DIR=%~dp0"
set "CLP_EXE=%TOOL_DIR%clp.exe"

if not exist "%CLP_EXE%" (
    echo [错误] 未在当前目录下找到 clp.exe
    echo 请确保本批处理文件与 clp.exe 位于同一文件夹。
    echo.
    pause
    exit /b 1
)

"%CLP_EXE%" --register

if errorlevel 1 (
    echo.
    echo [错误] 注册失败，请检查相关权限或杀毒软件拦截。
) else (
    echo.
    echo ============================================================
    echo [功能说明]
    echo   现在右键点击任意 .clp 文件，将展示 7-Zip 风格级联菜单：
    echo     1. 解压文件...       (弹出原生目录选择框，解压到指定位置)
    echo     2. 解压到当前目录    (直接解压输出到当前同级目录)
    echo     3. 解压到同名文件夹\ (解压到新建的同名子文件夹中)
    echo.
    echo   遇到同名文件时，会自动弹出确认提示框询问是否覆盖。
    echo.
    echo [命令行全局配置指引]
    echo   若需在任意终端直接输入 clp 运行，可将当前目录添加到用户 PATH：
    echo   当前目录: %TOOL_DIR%
    echo ============================================================
)

echo.
pause