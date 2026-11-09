use std::path::{Path, PathBuf};
use windows_sys::Win32::System::Console::FreeConsole;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNO,
};

/// 转换为 UTF-16 并以 NULL 结尾的宽字符数组
fn to_wide_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 释放控制台窗口（右键菜单/GUI 模式下避免黑框停留）
pub fn hide_console() {
    unsafe {
        FreeConsole();
    }
}

/// 弹出提示信息框 (OK)
pub fn msg_info(title: &str, content: &str) {
    let w_title = to_wide_str(title);
    let w_content = to_wide_str(content);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            w_content.as_ptr(),
            w_title.as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

/// 弹出错误信息框 (OK)
pub fn msg_error(title: &str, content: &str) {
    let w_title = to_wide_str(title);
    let w_content = to_wide_str(content);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            w_content.as_ptr(),
            w_title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// 弹出覆盖确认选择框 (Yes / No)
/// 返回 true 表示用户选择“覆盖”，false 表示取消/放弃
pub fn confirm_overwrite(target_path: &Path) -> bool {
    let title = "CLP 解压 - 文件冲突确认";
    let message = format!(
        "目标文件已存在：\n\n{}\n\n是否覆盖现有文件？\n\n[是] 覆盖并继续\n[否] 跳过并取消解压",
        target_path.display()
    );

    let w_title = to_wide_str(title);
    let w_message = to_wide_str(&message);

    unsafe {
        let ret = MessageBoxW(
            std::ptr::null_mut(),
            w_message.as_ptr(),
            w_title.as_ptr(),
            MB_YESNO | MB_ICONQUESTION,
        );
        ret == IDYES
    }
}

/// 弹出原生文件夹选择对话框，返回选中的文件夹路径
pub fn pick_folder(initial_dir: Option<&Path>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title("请选择 CLP 解压目标文件夹");
    if let Some(dir) = initial_dir {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_folder()
}
