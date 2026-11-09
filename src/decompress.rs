use std::fs::File;
use std::io::{self, BufReader, BufWriter, Cursor, Read, Write};
use std::path::Path;
use std::time::Instant;

pub const MAGIC_CLP3: [u8; 4] = [b'C', b'L', b'P', b'3'];
pub const MAGIC_CLP2: [u8; 4] = [b'C', b'L', b'P', b'2'];
pub const MAGIC_CLP1: [u8; 4] = [b'C', b'L', b'P', b'1'];
pub const MAGIC_CLP_IR: [u8; 4] = [0xFD, 0x2F, 0xB5, 0x30];

// XZ magic: 0xFD '7' 'z' 'X' 'Z' 0x00
pub const XZ_HEADER_MAGIC: [u8; 6] = [0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00];

pub const VAR_INT_PLACEHOLDER: u8 = 0x11;
pub const VAR_DICT_PLACEHOLDER: u8 = 0x12;
pub const VAR_FLOAT_PLACEHOLDER: u8 = 0x13;

pub const EIGHT_BYTE_ENCODED_FLOAT_DIGITS_BIT_MASK: u64 = (1u64 << 54) - 1;
pub const MAX_REPRESENTABLE_DIGITS_VALUE: u64 = 9_999_999_999_999_999;

// CLP IR 协议常量
const METADATA_ENCODING_JSON: u8 = 0x01;
const METADATA_LEN_UBYTE: u8 = 0x11;
const METADATA_LEN_USHORT: u8 = 0x12;
const PAYLOAD_EOF: u8 = 0x00;
const PAYLOAD_VAR_EIGHT_BYTE_ENCODING: u8 = 0x19;
const PAYLOAD_VAR_STR_LEN_UBYTE: u8 = 0x11;
const PAYLOAD_VAR_STR_LEN_USHORT: u8 = 0x12;
const PAYLOAD_VAR_STR_LEN_INT: u8 = 0x13;
const PAYLOAD_LOGTYPE_STR_LEN_UBYTE: u8 = 0x21;
const PAYLOAD_LOGTYPE_STR_LEN_USHORT: u8 = 0x22;
const PAYLOAD_LOGTYPE_STR_LEN_INT: u8 = 0x23;
const PAYLOAD_TIMESTAMP_VAL: u8 = 0x30;

#[derive(Debug, Clone)]
pub struct DecompressStats {
    pub total_lines: usize,
    pub uncompressed_size: u64,
    pub elapsed_ms: u128,
    pub format: &'static str,
}

/// 读取 VarInt (LEB128)
pub fn read_varint<R: Read>(reader: &mut R) -> io::Result<i32> {
    let mut val: u32 = 0;
    let mut shift = 0;
    loop {
        let mut b = [0u8; 1];
        reader.read_exact(&mut b)?;
        val |= ((b[0] & 0x7F) as u32) << shift;
        if (b[0] & 0x80) == 0 {
            break;
        }
        shift += 7;
        if shift > 35 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "VarInt overflow"));
        }
    }
    Ok(val as i32)
}

/// 读取 Java DataOutputStream.writeUTF 格式的字符串 (2 字节大端长度 + UTF-8 字节)
pub fn read_java_utf<R: Read>(reader: &mut R) -> io::Result<String> {
    let mut len_buf = [0u8; 2];
    reader.read_exact(&mut len_buf)?;
    let len = u16::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    String::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// 解码浮点数变量 (100% 对应 CLP 官方编码规范与 FloatEncodingTest)
pub fn decode_float_var(encoded_var: i64) -> Result<String, String> {
    let u_var = encoded_var as u64;
    let decimal_point_pos = ((u_var & 0x0F) + 1) as usize;
    let mut temp = u_var >> 4;
    let num_digits = ((temp & 0x0F) + 1) as usize;
    temp >>= 4;
    let mut digits = temp & EIGHT_BYTE_ENCODED_FLOAT_DIGITS_BIT_MASK;
    if digits > MAX_REPRESENTABLE_DIGITS_VALUE {
        return Err("Digits in encoded float are larger than max representable value.".into());
    }
    temp >>= 55;
    let is_negative = temp > 0;

    if num_digits < decimal_point_pos {
        return Err("Invalid decimal-point position in encoded float.".into());
    }

    let value_length = num_digits + 1 + if is_negative { 1 } else { 0 };
    let mut value = vec![0u8; value_length];
    let mut num_chars_to_process = value_length;

    if is_negative {
        value[0] = b'-';
        num_chars_to_process -= 1;
    }

    let mut pos = (value_length - 1) as isize;
    let decimal_point_pos_from_left = (value_length - 1 - decimal_point_pos) as isize;

    while pos > decimal_point_pos_from_left && digits > 0 {
        value[pos as usize] = b'0' + (digits % 10) as u8;
        digits /= 10;
        num_chars_to_process -= 1;
        pos -= 1;
    }

    if digits > 0 {
        if num_chars_to_process == 0 {
            return Err("Encoded number of digits doesn't match encoded digits in encoded float.".into());
        }
        pos -= 1;
        num_chars_to_process -= 1;

        while digits > 0 {
            if num_chars_to_process == 0 {
                return Err("Encoded number of digits doesn't match encoded digits in encoded float.".into());
            }
            value[pos as usize] = b'0' + (digits % 10) as u8;
            digits /= 10;
            num_chars_to_process -= 1;
            pos -= 1;
        }
    }

    while num_chars_to_process > 0 {
        value[pos as usize] = b'0';
        num_chars_to_process -= 1;
        pos -= 1;
    }

    value[decimal_point_pos_from_left as usize] = b'.';
    Ok(String::from_utf8_lossy(&value).to_string())
}

/// 逐行模板变量拼装渲染
pub fn decode_message(
    logtype: &[u8],
    dvs: &[String],
    evs: &[i64],
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let mut last_pos = 0;
    let mut dvs_idx = 0;
    let mut evs_idx = 0;

    for (i, &c) in logtype.iter().enumerate() {
        if c == VAR_FLOAT_PLACEHOLDER {
            out.extend_from_slice(&logtype[last_pos..i]);
            last_pos = i + 1;
            if evs_idx >= evs.len() {
                return Err("Too few encoded vars in log message".into());
            }
            let f_str = decode_float_var(evs[evs_idx])?;
            evs_idx += 1;
            out.extend_from_slice(f_str.as_bytes());
        } else if c == VAR_INT_PLACEHOLDER {
            out.extend_from_slice(&logtype[last_pos..i]);
            last_pos = i + 1;
            if evs_idx >= evs.len() {
                return Err("Too few encoded vars in log message".into());
            }
            let int_str = evs[evs_idx].to_string();
            evs_idx += 1;
            out.extend_from_slice(int_str.as_bytes());
        } else if c == VAR_DICT_PLACEHOLDER {
            out.extend_from_slice(&logtype[last_pos..i]);
            last_pos = i + 1;
            if dvs_idx >= dvs.len() {
                return Err("Too few dictionary vars in log message".into());
            }
            out.extend_from_slice(dvs[dvs_idx].as_bytes());
            dvs_idx += 1;
        }
    }

    if last_pos < logtype.len() {
        out.extend_from_slice(&logtype[last_pos..]);
    }
    Ok(())
}

/// CLP3 列式分流解压还原 (100% Bit-exact 无损)
fn decompress_clp3<R: Read, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<usize> {
    // 1. 读取模板字典
    let tmpl_count = read_varint(reader)? as usize;
    let mut templates = Vec::with_capacity(tmpl_count);
    for _ in 0..tmpl_count {
        let len = read_varint(reader)? as usize;
        let mut b = vec![0u8; len];
        reader.read_exact(&mut b)?;
        templates.push(b);
    }

    // 2. 读取每行模板 ID
    let line_count = read_varint(reader)? as usize;
    let mut tids = Vec::with_capacity(line_count);
    for _ in 0..line_count {
        tids.push(read_varint(reader)? as usize);
    }

    // 3. 读取字典变量列
    let mut dvs_lens = Vec::with_capacity(line_count);
    for _ in 0..line_count {
        dvs_lens.push(read_varint(reader)? as usize);
    }

    let mut all_dvs: Vec<Vec<String>> = Vec::with_capacity(line_count);
    for i in 0..line_count {
        let len = dvs_lens[i];
        if len > 0 {
            let mut row = Vec::with_capacity(len);
            for _ in 0..len {
                let slen = read_varint(reader)? as usize;
                let mut b = vec![0u8; slen];
                reader.read_exact(&mut b)?;
                row.push(String::from_utf8(b).unwrap_or_default());
            }
            all_dvs.push(row);
        } else {
            all_dvs.push(Vec::new());
        }
    }

    // 4. 读取数值变量列
    let mut evs_lens = Vec::with_capacity(line_count);
    for _ in 0..line_count {
        evs_lens.push(read_varint(reader)? as usize);
    }

    let mut all_evs: Vec<Vec<i64>> = Vec::with_capacity(line_count);
    for i in 0..line_count {
        let len = evs_lens[i];
        if len > 0 {
            let mut row = Vec::with_capacity(len);
            for _ in 0..len {
                let mut b = [0u8; 8];
                reader.read_exact(&mut b)?;
                row.push(i64::from_be_bytes(b));
            }
            all_evs.push(row);
        } else {
            all_evs.push(Vec::new());
        }
    }

    // 5. 逐行还原输出 (跨平台统一 LF 换行)
    let mut line_buf = Vec::new();
    for i in 0..line_count {
        line_buf.clear();
        let tid = tids[i];
        if tid >= templates.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Template ID {} out of bounds (max {})", tid, templates.len()),
            ));
        }
        decode_message(&templates[tid], &all_dvs[i], &all_evs[i], &mut line_buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        writer.write_all(&line_buf)?;
        writer.write_all(b"\n")?;
    }

    Ok(line_count)
}

/// CLP2 列式分流解压还原
fn decompress_clp2<R: Read, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<usize> {
    let mut int_buf = [0u8; 4];

    reader.read_exact(&mut int_buf)?;
    let tmpl_count = i32::from_be_bytes(int_buf) as usize;
    let mut templates = Vec::with_capacity(tmpl_count);
    for _ in 0..tmpl_count {
        templates.push(read_java_utf(reader)?.into_bytes());
    }

    reader.read_exact(&mut int_buf)?;
    let line_count = i32::from_be_bytes(int_buf) as usize;
    let mut tids = Vec::with_capacity(line_count);
    for _ in 0..line_count {
        reader.read_exact(&mut int_buf)?;
        tids.push(i32::from_be_bytes(int_buf) as usize);
    }

    let mut byte_buf = [0u8; 1];
    let mut dvs_lens = Vec::with_capacity(line_count);
    for _ in 0..line_count {
        reader.read_exact(&mut byte_buf)?;
        dvs_lens.push(byte_buf[0] as usize);
    }

    let mut all_dvs = Vec::with_capacity(line_count);
    for i in 0..line_count {
        let len = dvs_lens[i];
        if len > 0 {
            let mut row = Vec::with_capacity(len);
            for _ in 0..len {
                row.push(read_java_utf(reader)?);
            }
            all_dvs.push(row);
        } else {
            all_dvs.push(Vec::new());
        }
    }

    let mut evs_lens = Vec::with_capacity(line_count);
    for _ in 0..line_count {
        reader.read_exact(&mut byte_buf)?;
        evs_lens.push(byte_buf[0] as usize);
    }

    let mut all_evs = Vec::with_capacity(line_count);
    for i in 0..line_count {
        let len = evs_lens[i];
        if len > 0 {
            let mut row = Vec::with_capacity(len);
            for _ in 0..len {
                let mut long_buf = [0u8; 8];
                reader.read_exact(&mut long_buf)?;
                row.push(i64::from_be_bytes(long_buf));
            }
            all_evs.push(row);
        } else {
            all_evs.push(Vec::new());
        }
    }

    let mut line_buf = Vec::new();
    for i in 0..line_count {
        line_buf.clear();
        let tid = tids[i];
        decode_message(&templates[tid], &all_dvs[i], &all_evs[i], &mut line_buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        writer.write_all(&line_buf)?;
        writer.write_all(b"\n")?;
    }

    Ok(line_count)
}

/// CLP1 历史格式解压还原
fn decompress_clp1<R: Read, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<usize> {
    let mut int_buf = [0u8; 4];
    reader.read_exact(&mut int_buf)?;
    let tmpl_count = i32::from_be_bytes(int_buf) as usize;
    let mut templates = Vec::with_capacity(tmpl_count);
    for _ in 0..tmpl_count {
        templates.push(read_java_utf(reader)?.into_bytes());
    }

    reader.read_exact(&mut int_buf)?;
    let line_count = i32::from_be_bytes(int_buf) as usize;
    let mut tids = Vec::with_capacity(line_count);
    for _ in 0..line_count {
        reader.read_exact(&mut int_buf)?;
        tids.push(i32::from_be_bytes(int_buf) as usize);
    }

    let mut short_buf = [0u8; 2];
    let mut long_buf = [0u8; 8];
    let mut line_buf = Vec::new();

    for i in 0..line_count {
        line_buf.clear();
        reader.read_exact(&mut short_buf)?;
        let dv_count = u16::from_be_bytes(short_buf) as usize;
        let mut dvs = Vec::with_capacity(dv_count);
        for _ in 0..dv_count {
            dvs.push(read_java_utf(reader)?);
        }

        reader.read_exact(&mut short_buf)?;
        let ev_count = u16::from_be_bytes(short_buf) as usize;
        let mut evs = Vec::with_capacity(ev_count);
        for _ in 0..ev_count {
            reader.read_exact(&mut long_buf)?;
            evs.push(i64::from_be_bytes(long_buf));
        }

        let tid = tids[i];
        decode_message(&templates[tid], &dvs, &evs, &mut line_buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        writer.write_all(&line_buf)?;
        writer.write_all(b"\n")?;
    }

    Ok(line_count)
}

/// 官方单流交织 IR 协议解压还原
fn decompress_official_ir<R: Read, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<usize> {
    let mut byte_buf = [0u8; 1];
    reader.read_exact(&mut byte_buf)?;
    let meta_encoding = byte_buf[0];
    if meta_encoding != METADATA_ENCODING_JSON {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Unsupported metadata encoding tag: 0x{:02X}", meta_encoding),
        ));
    }

    reader.read_exact(&mut byte_buf)?;
    let len_tag = byte_buf[0];
    let meta_len: usize = if len_tag == METADATA_LEN_UBYTE {
        reader.read_exact(&mut byte_buf)?;
        byte_buf[0] as usize
    } else if len_tag == METADATA_LEN_USHORT {
        let mut short_buf = [0u8; 2];
        reader.read_exact(&mut short_buf)?;
        u16::from_be_bytes(short_buf) as usize
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Unsupported metadata length tag: 0x{:02X}", len_tag),
        ));
    };

    let mut meta_bytes = vec![0u8; meta_len];
    reader.read_exact(&mut meta_bytes)?;

    let mut encoded_vars = Vec::new();
    let mut dict_vars = Vec::new();
    let mut line_count = 0;
    let mut line_buf = Vec::new();

    loop {
        let mut tag_buf = [0u8; 1];
        if reader.read_exact(&mut tag_buf).is_err() {
            break;
        }
        let tag = tag_buf[0];
        if tag == PAYLOAD_EOF {
            break;
        }

        if tag == PAYLOAD_VAR_EIGHT_BYTE_ENCODING {
            let mut long_buf = [0u8; 8];
            reader.read_exact(&mut long_buf)?;
            encoded_vars.push(i64::from_be_bytes(long_buf));
        } else if tag == PAYLOAD_VAR_STR_LEN_UBYTE {
            reader.read_exact(&mut byte_buf)?;
            let len = byte_buf[0] as usize;
            let mut b = vec![0u8; len];
            reader.read_exact(&mut b)?;
            dict_vars.push(String::from_utf8(b).unwrap_or_default());
        } else if tag == PAYLOAD_VAR_STR_LEN_USHORT {
            let mut short_buf = [0u8; 2];
            reader.read_exact(&mut short_buf)?;
            let len = u16::from_be_bytes(short_buf) as usize;
            let mut b = vec![0u8; len];
            reader.read_exact(&mut b)?;
            dict_vars.push(String::from_utf8(b).unwrap_or_default());
        } else if tag == PAYLOAD_VAR_STR_LEN_INT {
            let mut int_buf = [0u8; 4];
            reader.read_exact(&mut int_buf)?;
            let len = i32::from_be_bytes(int_buf) as usize;
            let mut b = vec![0u8; len];
            reader.read_exact(&mut b)?;
            dict_vars.push(String::from_utf8(b).unwrap_or_default());
        } else if tag == PAYLOAD_LOGTYPE_STR_LEN_UBYTE
            || tag == PAYLOAD_LOGTYPE_STR_LEN_USHORT
            || tag == PAYLOAD_LOGTYPE_STR_LEN_INT
        {
            let lt_len: usize = if tag == PAYLOAD_LOGTYPE_STR_LEN_UBYTE {
                reader.read_exact(&mut byte_buf)?;
                byte_buf[0] as usize
            } else if tag == PAYLOAD_LOGTYPE_STR_LEN_USHORT {
                let mut short_buf = [0u8; 2];
                reader.read_exact(&mut short_buf)?;
                u16::from_be_bytes(short_buf) as usize
            } else {
                let mut int_buf = [0u8; 4];
                reader.read_exact(&mut int_buf)?;
                i32::from_be_bytes(int_buf) as usize
            };

            let mut lt_bytes = vec![0u8; lt_len];
            reader.read_exact(&mut lt_bytes)?;

            line_buf.clear();
            decode_message(&lt_bytes, &dict_vars, &encoded_vars, &mut line_buf)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            writer.write_all(&line_buf)?;
            writer.write_all(b"\n")?;

            line_count += 1;
            encoded_vars.clear();
            dict_vars.clear();
        } else if tag == PAYLOAD_TIMESTAMP_VAL {
            let mut long_buf = [0u8; 8];
            reader.read_exact(&mut long_buf)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unknown CLP IR tag: 0x{:02X}", tag),
            ));
        }
    }

    Ok(line_count)
}

/// 执行 CLP 文件解压操作
/// @param src_path 来源 .clp 归档文件
/// @param dst_path 目标解压输出文件
pub fn decompress_file(src_path: &Path, dst_path: &Path) -> io::Result<DecompressStats> {
    let start_time = Instant::now();

    if !src_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("源文件不存在: {}", src_path.display()),
        ));
    }

    // 确保目标文件的父目录存在
    if let Some(parent) = dst_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let src_file = File::open(src_path)?;
    let mut src_reader = BufReader::with_capacity(65536, src_file);

    // 检查是否具备 XZ 压缩签名 (0xFD, '7', 'z', 'X', 'Z', 0x00)
    let mut header_check = [0u8; 6];
    let n = src_reader.read(&mut header_check)?;

    // 将游标复位回开头
    use std::io::Seek;
    src_reader.seek(io::SeekFrom::Start(0))?;

    let is_xz = n >= 6 && header_check == XZ_HEADER_MAGIC;

    let decompressed_raw_bytes: Vec<u8> = if is_xz {
        let mut decompressed = Vec::new();
        lzma_rs::xz_decompress(&mut src_reader, &mut decompressed)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("XZ 解压失败: {}", e)))?;
        decompressed
    } else {
        let mut raw = Vec::new();
        src_reader.read_to_end(&mut raw)?;
        raw
    };

    if decompressed_raw_bytes.len() < 4 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "文件内容过短，缺少有效 CLP 魔数头",
        ));
    }

    let mut cursor = Cursor::new(&decompressed_raw_bytes);
    let mut magic = [0u8; 4];
    cursor.read_exact(&mut magic)?;

    let dst_file = File::create(dst_path)?;
    let mut writer = BufWriter::with_capacity(65536, dst_file);

    let (line_count, format_name) = if magic == MAGIC_CLP3 {
        (decompress_clp3(&mut cursor, &mut writer)?, "CLP3 (工业级列式分流)")
    } else if magic == MAGIC_CLP2 {
        (decompress_clp2(&mut cursor, &mut writer)?, "CLP2 (列式分流)")
    } else if magic == MAGIC_CLP1 {
        (decompress_clp1(&mut cursor, &mut writer)?, "CLP1 (传统行式)")
    } else if magic == MAGIC_CLP_IR {
        (decompress_official_ir(&mut cursor, &mut writer)?, "CLP Official IR")
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("未知或不受支持的 CLP 归档魔数: {:?}", magic),
        ));
    };

    writer.flush()?;

    let uncompressed_size = dst_path.metadata().map(|m| m.len()).unwrap_or(0);
    let elapsed_ms = start_time.elapsed().as_millis();

    Ok(DecompressStats {
        total_lines: line_count,
        uncompressed_size,
        elapsed_ms,
        format: format_name,
    })
}
