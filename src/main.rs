mod decompress;
mod gui;

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const VERSION: &str = "1.0.0";

/// 获取解压后的默认文件名
/// 例如:
///   "app.log.clp" -> "app.log"
///   "test.clp"    -> "test.log"
///   "mydata"      -> "mydata.log"
fn get_default_output_filename(input_path: &Path) -> String {
    let file_name = input_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "output.log".to_string());

    if let Some(stripped) = file_name.strip_suffix(".clp") {
        if stripped.contains('.') {
            stripped.to_string()
        } else {
            format!("{}.log", stripped)
        }
    } else {
        format!("{}.log", file_name)
    }
}

/// 获取同名文件夹名称（去除去掉 .clp 后的名称）
fn get_archive_dir_name(input_path: &Path) -> String {
    let file_name = input_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "clp_extracted".to_string());

    if let Some(stripped) = file_name.strip_suffix(".clp") {
        stripped.to_string()
    } else {
        file_name
    }
}

/// 解析目标输出文件路径
/// @param input_file 来源 clp 文件路径
/// @param output_arg 用户指定的输出参数（可为目录或具体文件名，相对或绝对路径，None 则为当前工作目录）
fn resolve_output_filepath(input_file: &Path, output_arg: Option<&str>) -> io::Result<PathBuf> {
    let default_name = get_default_output_filename(input_file);

    match output_arg {
        None => {
            let cwd = env::current_dir()?;
            Ok(cwd.join(default_name))
        }
        Some(arg) => {
            let p = PathBuf::from(arg);

            // 如果路径显式以分隔符结尾，或者已存在且为目录，则作为目录处理
            let ends_with_slash = arg.ends_with('\\') || arg.ends_with('/');
            if ends_with_slash || (p.exists() && p.is_dir()) {
                Ok(p.join(default_name))
            } else if p.extension().is_some() {
                // 有扩展名，判定为具体目标文件路径
                Ok(p)
            } else {
                // 没有扩展名且不存在，默认视作目标文件夹并放入默认文件名
                Ok(p.join(default_name))
            }
        }
    }
}

/// 打印命令行帮助说明
fn print_help() {
    println!(
        r#"CLP 高性能工业级日志解压工具 (Windows Native Edition) v{}

使用方法:
  clp <archive.clp> [options] [output_path]
  clp x <archive.clp> [-o<output_dir>] [-y]
  clp e <archive.clp> [-o<output_dir>] [-y]

命令:
  x, e              解压 CLP 归档（兼容 7-Zip 命令行调用语法）
  --register        注册 Windows 鼠标右键子菜单（无需管理员权限）
  --unregister      卸载 Windows 鼠标右键子菜单

选项:
  -o <path>         指定解压输出文件或目录（支持绝对路径与相对路径）
  -o<path>          紧凑指定解压输出目录（兼容 7-Zip 风格）
  -y, --yes, --force 遇到同名文件时直接覆盖，不进行确认询问
  -h, --help        显示此帮助信息
  -v, --version     显示版本信息

GUI / 右键集成选项:
  --gui-extract-here    解压到当前文件所在目录
  --gui-extract-dialog  弹出文件夹选择框选择解压位置
  --gui-extract-to-dir  解压到同名子文件夹

示例:
  clp app.2026-11-05.log.clp
  clp D:\logs\app.clp D:\target_dir\
  clp D:\logs\app.clp .\restored.log -y
  clp x D:\logs\app.clp -oD:\extracted -y
"#,
        VERSION
    );
}

/// 执行右键菜单注册
pub fn register_context_menu() -> io::Result<()> {
    let current_exe = env::current_exe()?;
    let exe_path = current_exe.to_string_lossy().replace('/', "\\");

    println!("正在注册 Windows 右键菜单 (当前执行程序: {})...", exe_path);

    // 1. 注册 SystemFileAssociations\.clp\shell\CLP
    let base_assoc = r"HKCU\Software\Classes\SystemFileAssociations\.clp\shell\CLP";
    run_reg(&["add", base_assoc, "/v", "MUIVerb", "/t", "REG_SZ", "/d", "CLP", "/f"])?;
    run_reg(&["add", base_assoc, "/v", "SubCommands", "/t", "REG_SZ", "/d", "", "/f"])?;
    run_reg(&["add", base_assoc, "/v", "Icon", "/t", "REG_SZ", "/d", &format!("\"{}\",0", exe_path), "/f"])?;

    // 子项 1: 解压文件...
    let menu1 = format!(r"{}\shell\menu1", base_assoc);
    run_reg(&["add", &menu1, "/v", "MUIVerb", "/t", "REG_SZ", "/d", "解压文件...", "/f"])?;
    run_reg(&["add", &menu1, "/v", "Icon", "/t", "REG_SZ", "/d", &format!("\"{}\",0", exe_path), "/f"])?;
    let menu1_cmd = format!(r"{}\command", menu1);
    run_reg(&["add", &menu1_cmd, "/ve", "/t", "REG_SZ", "/d", &format!("\"{}\" --gui-extract-dialog \"%1\"", exe_path), "/f"])?;

    // 子项 2: 解压到当前目录
    let menu2 = format!(r"{}\shell\menu2", base_assoc);
    run_reg(&["add", &menu2, "/v", "MUIVerb", "/t", "REG_SZ", "/d", "解压到当前目录", "/f"])?;
    run_reg(&["add", &menu2, "/v", "Icon", "/t", "REG_SZ", "/d", &format!("\"{}\",0", exe_path), "/f"])?;
    let menu2_cmd = format!(r"{}\command", menu2);
    run_reg(&["add", &menu2_cmd, "/ve", "/t", "REG_SZ", "/d", &format!("\"{}\" --gui-extract-here \"%1\"", exe_path), "/f"])?;

    // 子项 3: 解压到同名文件夹
    let menu3 = format!(r"{}\shell\menu3", base_assoc);
    run_reg(&["add", &menu3, "/v", "MUIVerb", "/t", "REG_SZ", "/d", "解压到同名文件夹\\", "/f"])?;
    run_reg(&["add", &menu3, "/v", "Icon", "/t", "REG_SZ", "/d", &format!("\"{}\",0", exe_path), "/f"])?;
    let menu3_cmd = format!(r"{}\command", menu3);
    run_reg(&["add", &menu3_cmd, "/ve", "/t", "REG_SZ", "/d", &format!("\"{}\" --gui-extract-to-dir \"%1\"", exe_path), "/f"])?;

    // 2. 注册文件类型关联 .clp -> CLP.Archive
    run_reg(&["add", r"HKCU\Software\Classes\.clp", "/ve", "/t", "REG_SZ", "/d", "CLP.Archive", "/f"])?;
    run_reg(&["add", r"HKCU\Software\Classes\CLP.Archive", "/ve", "/t", "REG_SZ", "/d", "CLP 压缩日志文件", "/f"])?;
    run_reg(&["add", r"HKCU\Software\Classes\CLP.Archive\DefaultIcon", "/ve", "/t", "REG_SZ", "/d", &format!("\"{}\",0", exe_path), "/f"])?;

    let clp_archive_shell = r"HKCU\Software\Classes\CLP.Archive\shell\open\command";
    run_reg(&["add", clp_archive_shell, "/ve", "/t", "REG_SZ", "/d", &format!("\"{}\" --gui-extract-dialog \"%1\"", exe_path), "/f"])?;

    println!(">>> 注册成功！现在可在任意 .clp 文件上右键体验 7-Zip 风格级联解压子菜单。");
    Ok(())
}

/// 执行右键菜单卸载
pub fn unregister_context_menu() -> io::Result<()> {
    println!("正在卸载 Windows 右键菜单...");
    let _ = run_reg(&["delete", r"HKCU\Software\Classes\SystemFileAssociations\.clp\shell\CLP", "/f"]);
    let _ = run_reg(&["delete", r"HKCU\Software\Classes\.clp", "/f"]);
    let _ = run_reg(&["delete", r"HKCU\Software\Classes\CLP.Archive", "/f"]);
    println!(">>> 卸载成功！右键菜单项已清除。");
    Ok(())
}

fn run_reg(args: &[&str]) -> io::Result<()> {
    let output = Command::new("reg").args(args).output()?;
    if !output.status.success() {
        let err_msg = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(io::ErrorKind::Other, err_msg.to_string()));
    }
    Ok(())
}

/// 处理 GUI 模式下的解压与弹窗反馈
fn handle_gui_extraction(input_path: &Path, target_file: &Path) {
    if !input_path.exists() {
        gui::msg_error(
            "CLP 解压失败",
            &format!("源文件不存在：\n\n{}", input_path.display()),
        );
        return;
    }

    // 冲突检查：重名时提供一次确认选择
    if target_file.exists() {
        let confirmed = gui::confirm_overwrite(target_file);
        if !confirmed {
            return;
        }
    }

    match decompress::decompress_file(input_path, target_file) {
        Ok(stats) => {
            let size_mb = stats.uncompressed_size as f64 / 1024.0 / 1024.0;
            let success_text = format!(
                "解压完成！\n\n源文件：{}\n目标文件：{}\n还原格式：{}\n还原行数：{} 行\n解压大小：{:.2} MB ({} 字节)\n耗时：{} ms",
                input_path.display(),
                target_file.display(),
                stats.format,
                stats.total_lines,
                size_mb,
                stats.uncompressed_size,
                stats.elapsed_ms
            );
            gui::msg_info("CLP 解压成功", &success_text);
        }
        Err(e) => {
            let err_text = format!(
                "解压过程中发生错误：\n\n源文件：{}\n目标文件：{}\n错误信息：{}",
                input_path.display(),
                target_file.display(),
                e
            );
            gui::msg_error("CLP 解压错误", &err_text);
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();

    // 1. 检查是否为 GUI 模式（右键菜单点击）
    if args.iter().any(|arg| arg.starts_with("--gui-")) {
        gui::hide_console();

        if args.len() < 3 {
            gui::msg_error("CLP 错误", "缺少必要的 CLP 文件入参。");
            return;
        }

        let mode = &args[1];
        let file_path = PathBuf::from(&args[2]);

        match mode.as_str() {
            "--gui-extract-here" => {
                let parent = file_path.parent().unwrap_or_else(|| Path::new("."));
                let default_name = get_default_output_filename(&file_path);
                let target_file = parent.join(default_name);
                handle_gui_extraction(&file_path, &target_file);
            }
            "--gui-extract-dialog" => {
                let initial_dir = file_path.parent();
                if let Some(selected_dir) = gui::pick_folder(initial_dir) {
                    let default_name = get_default_output_filename(&file_path);
                    let target_file = selected_dir.join(default_name);
                    handle_gui_extraction(&file_path, &target_file);
                }
            }
            "--gui-extract-to-dir" => {
                let parent = file_path.parent().unwrap_or_else(|| Path::new("."));
                let dir_name = get_archive_dir_name(&file_path);
                let target_dir = parent.join(dir_name);
                let default_name = get_default_output_filename(&file_path);
                let target_file = target_dir.join(default_name);
                handle_gui_extraction(&file_path, &target_file);
            }
            _ => {
                gui::msg_error("CLP 错误", &format!("未知的 GUI 命令: {}", mode));
            }
        }
        return;
    }

    // 2. 命令行调用
    if args.len() <= 1 {
        print_help();
        return;
    }

    let mut force_overwrite = false;
    let mut output_param: Option<String> = None;
    let mut input_param: Option<String> = None;
    let mut is_7z_mode = false;

    let mut idx = 1;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "-h" || arg == "--help" || arg == "/?" {
            print_help();
            return;
        } else if arg == "-v" || arg == "--version" {
            println!("clp version {}", VERSION);
            return;
        } else if arg == "--register" {
            if let Err(e) = register_context_menu() {
                eprintln!("注册右键菜单失败: {}", e);
                std::process::exit(1);
            }
            return;
        } else if arg == "--unregister" {
            if let Err(e) = unregister_context_menu() {
                eprintln!("卸载右键菜单失败: {}", e);
                std::process::exit(1);
            }
            return;
        } else if arg == "-y" || arg == "--yes" || arg == "--force" {
            force_overwrite = true;
        } else if arg == "x" || arg == "e" {
            is_7z_mode = true;
        } else if arg == "-o" {
            idx += 1;
            if idx < args.len() {
                output_param = Some(args[idx].clone());
            } else {
                eprintln!("错误: -o 选项后缺少目标路径参数。");
                std::process::exit(1);
            }
        } else if arg.starts_with("-o") && arg.len() > 2 {
            // 支持 -oD:\target 或 -o="D:\target"
            let val = arg[2..].trim_start_matches('=');
            output_param = Some(val.to_string());
        } else if input_param.is_none() {
            input_param = Some(arg.clone());
        } else if output_param.is_none() && !is_7z_mode {
            output_param = Some(arg.clone());
        } else {
            eprintln!("未知参数: {}", arg);
            std::process::exit(1);
        }
        idx += 1;
    }

    let input_str = match input_param {
        Some(s) => s,
        None => {
            eprintln!("错误: 未指定需要解压的 CLP 归档文件。\n使用 clp --help 查看帮助。");
            std::process::exit(1);
        }
    };

    let input_path = PathBuf::from(&input_str);
    if !input_path.exists() {
        eprintln!("错误: 源文件不存在 -> {}", input_path.display());
        std::process::exit(1);
    }

    let target_file = match resolve_output_filepath(&input_path, output_param.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("解析目标路径失败: {}", e);
            std::process::exit(1);
        }
    };

    // 冲突检查：重名时提供确认选择
    if target_file.exists() && !force_overwrite {
        print!(
            "目标文件已存在: {}\n是否覆盖此文件？[y/N]: ",
            target_file.display()
        );
        io::stdout().flush().unwrap();

        let mut input_line = String::new();
        if io::stdin().read_line(&mut input_line).is_ok() {
            let trimmed = input_line.trim().to_lowercase();
            if trimmed != "y" && trimmed != "yes" {
                println!("解压已取消。");
                return;
            }
        } else {
            eprintln!("未获得有效确认，解压已取消。");
            std::process::exit(1);
        }
    }

    println!("============================================================");
    println!(">>> 开始解压 CLP 日志归档...");
    println!("  源文件:   {}", fs::canonicalize(&input_path).unwrap_or(input_path.clone()).display());
    println!("  目标文件: {}", target_file.display());
    println!("============================================================");

    match decompress::decompress_file(&input_path, &target_file) {
        Ok(stats) => {
            let size_mb = stats.uncompressed_size as f64 / 1024.0 / 1024.0;
            println!(">>> 解压成功完成！");
            println!("  识别格式: {}", stats.format);
            println!("  还原行数: {} 行", stats.total_lines);
            println!("  还原大小: {:.2} MB ({} 字节)", size_mb, stats.uncompressed_size);
            println!("  总共耗时: {} ms", stats.elapsed_ms);
            println!("============================================================");
        }
        Err(e) => {
            eprintln!(">>> 解压失败: {}", e);
            std::process::exit(1);
        }
    }
}
