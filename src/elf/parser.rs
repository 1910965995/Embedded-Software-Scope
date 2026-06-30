use std::collections::HashMap;
use std::fs;
use std::path::Path;
use crate::pipeline::sample::ValueType;
use super::dwarf;
use super::tree;

/// ELF 解析上下文（解析后的完整结果）
pub struct ElfContext {
    pub file_path: String,
    pub variables: Vec<ElfVariable>,
}

#[derive(Debug, Clone)]
pub struct ElfVariable {
    pub name: String,
    pub path: String,
    pub address: u32,
    pub byte_size: u32,
    pub value_type: ValueType,
    pub parent_path: Option<String>,
    pub source_file: Option<String>,
}

/// ELF 符号原始信息
#[derive(Debug, Clone)]
pub struct ElfSymbolRaw {
    pub name: String,
    pub address: u32,
    pub size: u32,
}

/// ELF + DWARF 解析器
pub struct ElfParser;

impl ElfParser {
    /// 加载 ELF 文件，解析符号表和 DWARF 信息
    pub fn load<P: AsRef<Path>>(path: P) -> Result<ElfContext, String> {
        let path = path.as_ref();
        let data = fs::read(path).map_err(|e| format!("无法读取文件: {}", e))?;

        // 通过 ELF header 的 class 字段判断 32/64 位，避免重复 parse
        // ELF header 前 5 字节: magic(4) + class(1)，class=1 为 32 位，class=2 为 64 位
        let is_64 = if data.len() >= 5 {
            data[4] == 2 // ELFCLASS64
        } else {
            return Err("文件过短，不是有效的 ELF 文件".to_string());
        };

        if is_64 {
            Self::load_elf64(path, &data)
        } else {
            Self::load_elf32(path, &data)
        }
    }

    fn load_elf32(path: &Path, data: &[u8]) -> Result<ElfContext, String> {
        use object::read::elf::ElfFile32;

        let elf = ElfFile32::<object::Endianness>::parse(data)
            .map_err(|e| format!("ELF32 解析失败: {}", e))?;

        let elf_symbols = Self::extract_symbols_object(&elf)?;
        let sections = Self::extract_dwarf_sections(&elf)?;
        let variables = Self::resolve_variables(&elf_symbols, &sections)?;

        Ok(ElfContext {
            file_path: path.display().to_string(),
            variables,
        })
    }

    fn load_elf64(path: &Path, data: &[u8]) -> Result<ElfContext, String> {
        use object::read::elf::ElfFile64;

        let elf = ElfFile64::<object::Endianness>::parse(data)
            .map_err(|e| format!("ELF64 解析失败: {}", e))?;

        let elf_symbols = Self::extract_symbols_object(&elf)?;
        let sections = Self::extract_dwarf_sections(&elf)?;
        let variables = Self::resolve_variables(&elf_symbols, &sections)?;

        Ok(ElfContext {
            file_path: path.display().to_string(),
            variables,
        })
    }

    fn resolve_variables(
        elf_symbols: &[ElfSymbolRaw],
        sections: &HashMap<&str, &[u8]>,
    ) -> Result<Vec<ElfVariable>, String> {
        let dwarf_result = if let (Some(info), Some(abbrev)) =
            (sections.get(".debug_info"), sections.get(".debug_abbrev"))
        {
            let dr = dwarf::parse_dwarf_sections(
                info,
                abbrev,
                sections.get(".debug_str").copied().unwrap_or(&[]),
                sections.get(".debug_str_offsets").copied().unwrap_or(&[]),
                sections.get(".debug_line").copied().unwrap_or(&[]),
                sections.get(".debug_ranges").copied().unwrap_or(&[]),
                sections.get(".debug_rnglists").copied().unwrap_or(&[]),
                sections.get(".debug_addr").copied().unwrap_or(&[]),
            )?;
            log::info!(
                "DWARF 解析: {} 个变量, {} 个类型定义, {} 个源文件",
                dr.variables.len(), dr.types.len(), dr.source_files.len()
            );
            Some(dr)
        } else {
            log::info!("无 DWARF 调试信息, 使用纯符号表模式");
            None
        };

        let variables = if let Some(ref dwarf_res) = dwarf_result {
            tree::build_from_dwarf(elf_symbols, dwarf_res)?
        } else {
            tree::build_from_symbols_only(elf_symbols)?
        };

        log::info!("最终变量列表: {} 个 (展开后)", variables.len());

        Ok(variables)
    }

    /// 从 ELF .symtab 提取 OBJECT 类型的全局变量符号
    fn extract_symbols_object<Elf: object::read::elf::FileHeader>(
        elf: &object::read::elf::ElfFile<Elf>,
    ) -> Result<Vec<ElfSymbolRaw>, String> {
        use object::Object;
        use object::ObjectSymbol;

        let mut symbols = Vec::new();
        let mut total_syms = 0usize;
        let mut skipped_kind = 0usize;
        let mut skipped_undef = 0usize;
        let mut skipped_addr = 0usize;

        for symbol in elf.symbols() {
            total_syms += 1;
            if symbol.kind() != object::SymbolKind::Data {
                skipped_kind += 1;
                continue;
            }
            if symbol.is_undefined() {
                skipped_undef += 1;
                continue;
            }
            let name = match symbol.name() {
                Ok(n) if !n.is_empty() => n.to_string(),
                _ => continue,
            };
            let address = symbol.address() as u32;
            let size = symbol.size() as u32;
            if address == 0 {
                skipped_addr += 1;
                continue;
            }
            symbols.push(ElfSymbolRaw { name, address, size });
        }

        log::info!(
            "ELF 符号表: 总计 {} 个符号, Data 类型保留 {} 个 (跳过: 类型不符={}, undefined={}, addr=0={})",
            total_syms, symbols.len(), skipped_kind, skipped_undef, skipped_addr
        );

        Ok(symbols)
    }

    /// 提取 DWARF 相关 section 的原始字节（零拷贝，借用 elf 对象）
    fn extract_dwarf_sections<'elf, Elf: object::read::elf::FileHeader>(
        elf: &'elf object::read::elf::ElfFile<Elf>,
    ) -> Result<HashMap<&'elf str, &'elf [u8]>, String> {
        use object::Object;
        use object::ObjectSection;

        let mut sections = HashMap::new();
        let target_sections: &[&str] = &[
            ".debug_info", ".debug_abbrev", ".debug_str",
            ".debug_str_offsets", ".debug_line",
            ".debug_ranges", ".debug_rnglists",
            ".debug_addr",
        ];

        for section in elf.sections() {
            if let Ok(name) = section.name() {
                if target_sections.contains(&name) {
                    if let Ok(data) = section.data() {
                        sections.insert(name, data);
                    }
                }
            }
        }

        Ok(sections)
    }
}
