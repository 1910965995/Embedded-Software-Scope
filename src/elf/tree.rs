use std::collections::{HashMap, HashSet};
use super::dwarf::{DwarfResult, DwarfVarInfo, DwarfTypeInfo};
use super::types::map_dwarf_type;
use super::parser::{ElfSymbolRaw, ElfVariable};
use crate::pipeline::sample::ValueType;

/// 解析后的类型（沿 typedef/const/volatile 链找到最终类型）
enum ResolvedType {
    Base(String, u32),
    Struct(u64),
    Array(u64),
    Pointer(u32),
    Unknown,
}

/// 结合 ELF 符号表和 DWARF 类型信息构建变量树
pub fn build_from_dwarf(
    elf_symbols: &[ElfSymbolRaw],
    dwarf: &DwarfResult,
) -> Result<Vec<ElfVariable>, String> {
    // 1. 交叉验证：只保留 DWARF 变量中名字也出现在 ELF 符号表的
    let elf_names: HashSet<&str> = elf_symbols.iter()
        .map(|s| s.name.as_str())
        .collect();

    let valid_dwarf_vars: Vec<&DwarfVarInfo> = dwarf.variables.iter()
        .filter(|v| elf_names.contains(v.name.as_str()) && v.address != 0)
        .collect();

    log::info!(
        "交叉验证: ELF符号={}, DWARF变量={}, 通过验证={} (被过滤: 名称不匹配或地址为0)",
        elf_symbols.len(),
        dwarf.variables.len(),
        valid_dwarf_vars.len()
    );
    if !dwarf.variables.is_empty() && valid_dwarf_vars.is_empty() {
        log::warn!("DWARF 有 {} 个变量但无一个匹配 ELF 符号表, 请检查编译选项 (可能需要 -g 并保留符号表)", dwarf.variables.len());
    }

    // 2. 对每个变量，解析类型链并展开
    let mut result = Vec::new();
    let mut skipped = 0usize;
    for dv in &valid_dwarf_vars {
        let resolved = resolve_type(dv.type_offset, &dwarf.types);
        let before = result.len();
        expand_variable(dv, &resolved, &dwarf.types, "", &mut result);
        if result.len() == before {
            skipped += 1;
            log::warn!(
                "变量 '{}' 展开失败 (type_offset=0x{:X}), 跳过",
                dv.name, dv.type_offset
            );
        }
    }
    log::info!(
        "类型展开: 输入 {} 个变量, 输出 {} 个叶子, 跳过 {} 个",
        valid_dwarf_vars.len(), result.len(), skipped
    );

    // 3. 先按 (path, address) 排序，使相同条目相邻
    //    （dedup_by 仅移除相邻重复，必须先 sort 再 dedup）
    result.sort_by(|a, b| {
        a.path.cmp(&b.path)
            .then_with(|| a.address.cmp(&b.address))
    });

    // 4. 去重（同名同地址的变量只保留一个）
    result.dedup_by(|a, b| a.path == b.path && a.address == b.address);
    Ok(result)
}

/// 无 DWARF 时回退到纯符号表模式
pub fn build_from_symbols_only(
    elf_symbols: &[ElfSymbolRaw],
) -> Result<Vec<ElfVariable>, String> {
    let mut result = Vec::new();
    for sym in elf_symbols {
        if sym.address == 0 || sym.size == 0 {
            continue;
        }
        let value_type = match sym.size {
            8 => ValueType::Float,
            4 => ValueType::Float,   // 嵌入式常见 float
            2 => ValueType::Uint16,
            1 => ValueType::Uint8,
            _ => ValueType::Uint32,
        };
        result.push(ElfVariable {
            name: sym.name.clone(),
            path: sym.name.clone(),
            address: sym.address,
            byte_size: sym.size,
            value_type,
            parent_path: None,
            source_file: None,
        });
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

/// 沿 typedef/const/volatile 链找到最终类型
fn resolve_type(offset: u64, types: &HashMap<u64, DwarfTypeInfo>) -> ResolvedType {
    let mut current = offset;
    let mut depth = 0;
    loop {
        if depth > 32 {
            return ResolvedType::Unknown; // 防止无限循环
        }
        depth += 1;
        match types.get(&current) {
            Some(DwarfTypeInfo::Alias { target_offset, .. }) => {
                current = *target_offset;
            }
            Some(DwarfTypeInfo::Base { name, byte_size }) => {
                return ResolvedType::Base(name.clone(), *byte_size);
            }
            Some(DwarfTypeInfo::Struct { .. }) => {
                return ResolvedType::Struct(current);
            }
            Some(DwarfTypeInfo::Array { .. }) => {
                return ResolvedType::Array(current);
            }
            Some(DwarfTypeInfo::Pointer { byte_size }) => {
                return ResolvedType::Pointer(*byte_size);
            }
            None => {
                return ResolvedType::Unknown;
            }
        }
    }
}

/// 将变量展开为叶子条目
fn expand_variable(
    dv: &DwarfVarInfo,
    resolved: &ResolvedType,
    types: &HashMap<u64, DwarfTypeInfo>,
    parent_path: &str,
    output: &mut Vec<ElfVariable>,
) {
    let path = if parent_path.is_empty() {
        dv.name.clone()
    } else {
        format!("{}.{}", parent_path, dv.name)
    };

    match resolved {
        ResolvedType::Base(name, byte_size) => {
            let vt = map_dwarf_type(name, *byte_size);
            output.push(ElfVariable {
                name: dv.name.clone(),
                path,
                address: dv.address,
                byte_size: *byte_size,
                value_type: vt,
                parent_path: if parent_path.is_empty() { None } else { Some(parent_path.to_string()) },
                source_file: dv.source_file.clone(),
            });
        }

        ResolvedType::Struct(type_offset) => {
            if let Some(DwarfTypeInfo::Struct { members, .. }) = types.get(type_offset) {
                // 限制展开深度，防止递归过深
                if parent_path.matches('.').count() < 4 {
                    for member in members {
                        let member_type = resolve_type(member.type_offset, types);
                        let member_dv = DwarfVarInfo {
                            name: member.name.clone(),
                            address: dv.address + member.member_offset,
                            type_offset: member.type_offset,
                            source_file: dv.source_file.clone(),
                            source_line: dv.source_line,
                        };
                        expand_variable(&member_dv, &member_type, types, &path, output);
                    }
                }
            }
        }

        ResolvedType::Array(type_offset) => {
            if let Some(DwarfTypeInfo::Array { element_type_offset, element_count, .. }) = types.get(type_offset) {
                let elem_type = resolve_type(*element_type_offset, types);
                let elem_size = get_type_byte_size(&elem_type, types);
                // 限制数组展开数量（避免超大数组）
                let count = (*element_count).min(256);
                for i in 0..count {
                    let elem_dv = DwarfVarInfo {
                        name: format!("[{}]", i),
                        address: dv.address + (i as u32) * elem_size,
                        type_offset: *element_type_offset,
                        source_file: dv.source_file.clone(),
                        source_line: dv.source_line,
                    };
                    let elem_path = format!("{}[{}]", path, i);
                    expand_variable(
                        &DwarfVarInfo { name: elem_path, ..elem_dv },
                        &elem_type, types, "",
                        output,
                    );
                }
            }
        }

        ResolvedType::Pointer(byte_size) => {
            // 指针类型：作为 uint32 显示
            output.push(ElfVariable {
                name: dv.name.clone(),
                path,
                address: dv.address,
                byte_size: *byte_size,
                value_type: ValueType::Uint32,
                parent_path: if parent_path.is_empty() { None } else { Some(parent_path.to_string()) },
                source_file: dv.source_file.clone(),
            });
        }

        ResolvedType::Unknown => {
            // 无法解析类型，使用默认 uint32 保留变量（不丢弃）
            output.push(ElfVariable {
                name: dv.name.clone(),
                path,
                address: dv.address,
                byte_size: 4,
                value_type: ValueType::Uint32,
                parent_path: if parent_path.is_empty() { None } else { Some(parent_path.to_string()) },
                source_file: dv.source_file.clone(),
            });
        }
    }
}

/// 获取类型的字节大小
fn get_type_byte_size(resolved: &ResolvedType, types: &HashMap<u64, DwarfTypeInfo>) -> u32 {
    match resolved {
        ResolvedType::Base(_, size) => *size,
        ResolvedType::Struct(offset) => {
            if let Some(DwarfTypeInfo::Struct { byte_size, .. }) = types.get(offset) {
                *byte_size
            } else {
                4
            }
        }
        ResolvedType::Array(offset) => {
            if let Some(DwarfTypeInfo::Array { byte_size, .. }) = types.get(offset) {
                *byte_size
            } else {
                4
            }
        }
        ResolvedType::Pointer(size) => *size,
        ResolvedType::Unknown => 4,
    }
}
