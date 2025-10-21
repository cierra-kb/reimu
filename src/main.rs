mod binreader;

use binreader::BinReader;
use elf::{endian::LittleEndian, symbol::SymbolTable};
use serde::Serialize;
use std::collections::HashMap;

const SHT_DYNSYM: u32 = 0xb;
const SHT_STRTAB: u32 = 0x3;

#[derive(Debug, Default, Serialize)]
struct Class {
    pub name: String,
    pub base: Vec<Class>,
}

impl Class {
    pub fn push_base(&mut self) -> &mut Class {
        self.base.push(Class::default());
        self.base.last_mut().unwrap()
    }

    pub fn get_display(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        self._get_display(&mut lines, 0);
        lines.join("\n")
    }

    fn _get_display(&self, buf: &mut Vec<String>, mut level: u32) {
        let demangled = cpp_demangle::Symbol::new(&self.name)
            .expect("failed to parse symbol")
            .demangle()
            .expect("failed to demangle symbol");
        let line = format!(
            "{}{}",
            std::iter::repeat(" ")
                .take(4 * level as usize)
                .collect::<String>(),
            demangled
        );
        buf.push(line);

        level += 1;

        for base in &self.base {
            base._get_display(buf, level);
        }
    }
}

fn get_section_range(data: &Vec<u8>, search_name: &String) -> Option<(u64, u64)> {
    let elf = elf::ElfBytes::<elf::endian::LittleEndian>::minimal_parse(data).unwrap();
    let (shdrs_r, strtab_r) = elf.section_headers_with_strtab().unwrap();
    let (shdrs, strtab) = (shdrs_r.unwrap(), strtab_r.unwrap());

    while let Some(header) = shdrs
        .into_iter()
        .filter(|header| match strtab.get(header.sh_name as usize) {
            Ok(header_name) => header_name == search_name,
            Err(_) => false,
        })
        .next()
    {
        return Some((header.sh_offset, header.sh_offset + header.sh_size));
    }

    None
}

fn dump_symbols(data: &Vec<u8>) -> (HashMap<String, u32>, HashMap<u32, String>) {
    let elf = elf::ElfBytes::<elf::endian::LittleEndian>::minimal_parse(data).unwrap();
    let shdrs = elf.section_headers().unwrap();

    let dynsym_section = shdrs
        .iter()
        .filter(|hdr| hdr.sh_type == SHT_DYNSYM)
        .next()
        .expect("no SHT_DYNSYM");
    let string_table_section = shdrs
        .iter()
        .filter(|hdr| hdr.sh_type == SHT_STRTAB)
        .next()
        .unwrap();
    let string_table = elf.section_data_as_strtab(&string_table_section).unwrap();

    let mut sym_addr_map: HashMap<String, u32> = HashMap::default();
    let mut addr_sym_map: HashMap<u32, String> = HashMap::default();

    SymbolTable::new(
        LittleEndian,
        elf::file::Class::ELF32,
        &data[dynsym_section.sh_offset as usize
            ..dynsym_section.sh_offset as usize + dynsym_section.sh_size as usize],
    )
    .iter()
    .for_each(|sym| {
        sym_addr_map.insert(
            string_table.get(sym.st_name as usize).unwrap().to_string(),
            sym.st_value as u32,
        );
        addr_sym_map.insert(
            sym.st_value as u32,
            string_table.get(sym.st_name as usize).unwrap().to_string(),
        );
    });

    return (sym_addr_map, addr_sym_map);
}

fn handle_typename(
    reader: &mut BinReader,
    output: &mut Class,
    offset: u32,
    start_data_rel_ro: u32,
    rtti_class_offsets: &Vec<u32>,
) {
    reader.set_position(offset + 4);

    let name = reader.read_cstr().expect("failed to read type name");
    let second_field = reader
        .read_u32()
        .expect("failed to read dword after type name");

    output.name = name;

    if rtti_class_offsets
        .iter()
        .find(|offset| **offset == second_field)
        .is_some()
    {
        // some typeinfos will only provide a reference to the type class
        // and the type name (see _ZTIN7cocos2d15CCTouchDelegateE).
        // we cannot just check if second_field is above the start address
        // of .data.rel.ro because another typeinfo is declared right after.
        return;
    }

    if second_field > start_data_rel_ro {
        // ; reference to rtti's type class
        // ; type name
        // ; parent typename < second_field
        // reference: _ZTIN7cocos2d10CCMenuItemE (1.3)
        handle_typename(
            reader,
            output.push_base(),
            second_field,
            start_data_rel_ro,
            rtti_class_offsets,
        );
    } else {
        let third_field = reader
            .read_u32()
            .expect("failed to read dword after second field");

        if third_field > start_data_rel_ro {
            // ; reference to rtti's type class
            // ; type name
            // -- new vtable --
            // vtable's offset to this = 0x00000000 < second_field
            // class's typeinfo < third_field
            //
            // we are already over the typeinfo we wanted to read.
            // reference: _ZTI17TextInputDelegate (1.3)
            return;
        } else {
            // ; reference to rtti's type class
            // ; type name
            // ; attribute < second_field
            // ; count of base classes < third_field
            // .. [base classes]
            //
            // base class:
            //     ; base class type info
            //     ; base class attributes
            // reference: _ZTIN7cocos2d7CCLayerE (1.3)

            let _attribute = second_field;
            let base_class_count = third_field;

            for _ in 0..base_class_count {
                let type_descriptor = reader
                    .read_u32()
                    .expect("failed to read offset to base class typeinfo");
                let _base_attribute = reader
                    .read_u32()
                    .expect("failed to read attribute of base class");
                let return_offset = reader.get_position();
                handle_typename(
                    reader,
                    output.push_base(),
                    type_descriptor,
                    start_data_rel_ro,
                    rtti_class_offsets,
                );
                reader.set_position(return_offset.try_into().unwrap());
            }
        }
    }
}

fn get_vtable_mangled_name(class_name: &String) -> String {
    if !class_name.find("::").is_some() {
        return format!("_ZTV{}{}", class_name.len(), class_name);
    }
    let mut mangled = "_ZTVN".to_string();

    for name in class_name.split("::") {
        mangled.push_str(format!("{}{}", name.len(), name).as_str());
    }

    mangled.push_str("E");
    mangled.to_string()
}

fn handle_vtable(
    reader: &mut BinReader,
    class_typeinfo: u32,
    cxxabi_offsets: &Vec<u32>,
) -> (i32, Vec<u32>) {
    let offset_to_this = reader.read_i32().unwrap();
    reader.set_position_relative(4); // skip reference to typeinfo

    let mut function_pointers = Vec::new();

    while let Some(addr) = reader.read_u32() {
        let next_u32 = reader.read_u32().expect("failed to read ahead");

        let in_typeinfo = cxxabi_offsets
            .iter()
            .find(|offset| **offset == next_u32)
            .is_some();
        let in_offset_to_this = next_u32 == class_typeinfo;

        if in_typeinfo || in_offset_to_this || addr == 0 {
            reader.set_position_relative(-8);
            break;
        }
        reader.set_position_relative(-4);

        function_pointers.push(addr);
    }

    (offset_to_this, function_pointers)
}

fn get_class_vtable(
    reader: &mut BinReader,
    vtable_addr: u32,
    cxxabi_offsets: Vec<u32>,
) -> Vec<(i32, Vec<u32>)> {
    let mut result: Vec<(i32, Vec<u32>)> = Vec::new();

    reader.set_position(vtable_addr + 4);
    let class_typeinfo = reader.read_u32().unwrap();
    reader.set_position(vtable_addr);

    let mut table_offset = vtable_addr;

    loop {
        reader.set_position(table_offset + 4);
        let typeinfo_addr = reader.read_u32().unwrap();
        reader.set_position(table_offset);

        if typeinfo_addr != class_typeinfo {
            break;
        }

        let table = handle_vtable(reader, class_typeinfo, &cxxabi_offsets);
        result.push(table);
        table_offset = reader.get_position() as u32;
    }

    result
}

fn main() {
    let cmd = clap::Command::new("reimu")
        .subcommand(
            clap::command!("class_info")
                .group(
                    clap::ArgGroup::new("actions")
                        .args(["dump-vtable-ida", "dump-vtable-json", "inheritance"])
                        .required(true),
                )
                .arg(clap::arg!(--"dump-vtable-ida"))
                .arg(clap::arg!(--"inheritance"))
                .arg(clap::arg!(--"dump-vtable-json"))
                .arg(
                    clap::arg!(-L --"library-path" <PATH>)
                        .value_parser(clap::value_parser!(std::path::PathBuf))
                        .required(true),
                )
                .arg(
                    clap::arg!(<CLASS> "The class name (case sensitive) (e.g. FLAlertLayer, cocos2d::CCNode)")
                        .required(true),
                ),
        )
        .subcommand(
            clap::command!("symbols")
                .arg(clap::arg!(-L --"library-path" <PATH>)
                    .value_parser(clap::value_parser!(std::path::PathBuf))
                    .required(true)
                )
        );

    match cmd.get_matches().subcommand() {
        Some(("symbols", matches)) => {
            let game_bin_path = matches
                .get_one::<std::path::PathBuf>("library-path")
                .unwrap();
            let game_bin = std::fs::read(game_bin_path)
                .expect(format!("failed to read given path: {:?}", game_bin_path).as_str());
            println!(
                "{}",
                serde_json::to_string_pretty(&dump_symbols(&game_bin)).unwrap()
            );
        }
        Some(("class_info", matches)) => {
            let game_bin_path = matches
                .get_one::<std::path::PathBuf>("library-path")
                .unwrap();
            let game_bin = std::fs::read(game_bin_path)
                .expect(format!("failed to read given path: {:?}", game_bin_path).as_str());
            let mut reader = BinReader::new(&game_bin);

            let (sym_to_addr, addr_to_sym) = dump_symbols(&game_bin);
            let action = matches.get_one::<clap::Id>("actions").unwrap().as_str();
            let class_name = matches.get_one::<String>("CLASS").unwrap();

            let cxxabi_offsets = vec![
                sym_to_addr["_ZTVN10__cxxabiv120__si_class_type_infoE"] + 8,
                sym_to_addr["_ZTVN10__cxxabiv117__class_type_infoE"] + 8,
                sym_to_addr["_ZTVN10__cxxabiv121__vmi_class_type_infoE"] + 8,
            ];

            match action {
                "inheritance" => {
                    let mut inherit_info = Class::default();
                    let vtable_symbol = get_vtable_mangled_name(class_name);
                    let vtable_addr = sym_to_addr
                        .get(&vtable_symbol)
                        .expect(format!("unknown symbol for vtable: {:?}", vtable_symbol).as_str());

                    reader.set_position(vtable_addr + 4);
                    let typeinfo_addr = reader.read_u32().unwrap();
                    reader.set_position(*vtable_addr);

                    handle_typename(
                        &mut reader,
                        &mut inherit_info,
                        typeinfo_addr,
                        get_section_range(&game_bin, &".data.rel.ro".to_string())
                            .unwrap()
                            .0 as u32,
                        &cxxabi_offsets,
                    );

                    println!("{}", inherit_info.get_display());
                }
                "dump-vtable-ida" | "dump-vtable-json" => {
                    #[derive(Serialize)]
                    struct DumpVtableJSONOutput {
                        name: String,
                        address: u32,
                        offset: u32,
                    }

                    let vtable_symbol = get_vtable_mangled_name(class_name);
                    let vtable_addr = sym_to_addr
                        .get(&vtable_symbol)
                        .expect(format!("unknown symbol for vtable: {:?}", vtable_symbol).as_str());
                    let dump = get_class_vtable(&mut reader, *vtable_addr, cxxabi_offsets);

                    if action == "dump-vtable-json" {
                        let mut entry: Vec<DumpVtableJSONOutput> = Vec::new();
                        let mut i = 0;
                        dump[0].1.iter().for_each(|addr| {
                            entry.push(DumpVtableJSONOutput {
                                name: (&addr_to_sym[addr]).clone(),
                                address: *addr,
                                offset: 8 + (4 * i),
                            });
                            i += 1;
                        });
                        println!("{}", serde_json::to_string_pretty(&entry).unwrap());
                    } else {
                        let mut main_class_fields: Vec<String> = Vec::new();
                        let mut last_offset_to_this = 0;
                        let mut filler_counter = 0;

                        for table in dump {
                            let offset_to_this = table.0.abs();
                            let vft_struct_name = format!("{}_{}_vft", class_name, offset_to_this);
                            let filler_size = (offset_to_this - last_offset_to_this) - 4;

                            if filler_size > 0 {
                                main_class_fields
                                    .push(format!("char fill_{}[{}]", filler_counter, filler_size));
                                filler_counter += 1;
                            }
                            main_class_fields
                                .push(format!("{}* __vtable_{}", vft_struct_name, offset_to_this));

                            last_offset_to_this = offset_to_this;

                            let mut function_name_counter: HashMap<String, u32> = HashMap::new();

                            println!("struct {} {{", vft_struct_name);

                            table.1.iter().for_each(|addr| {
                                let symbol = addr_to_sym[addr].to_owned();

                                if symbol.ends_with("D1Ev") {
                                    println!("    void (*__dtor)({}*);", class_name);
                                } else if symbol.ends_with("D0Ev") {
                                    println!("    void (*__delete)({}*);", class_name);
                                } else {
                                    let mut demangled = cpp_demangle::Symbol::new(&symbol)
                                        .expect("failed to parse symbol")
                                        .demangle()
                                        .expect("failed to demangle symbol");

                                    if demangled.starts_with("{virtual override thunk") {
                                        demangled =
                                            demangled.split_once(",").unwrap().1[1..].to_string();
                                        demangled = demangled[0..demangled.len() - 2].to_string();
                                    }

                                    let start_of_args = demangled.find("(").unwrap();

                                    let mut name = demangled[0..start_of_args]
                                        .split("::")
                                        .last()
                                        .unwrap()
                                        .to_string();

                                    if function_name_counter.contains_key(&name) {
                                        function_name_counter
                                            .insert(name.clone(), function_name_counter[&name] + 1);
                                        name = format!("{}_{}", name, function_name_counter[&name]);
                                    } else {
                                        function_name_counter.insert(name.to_owned(), 1);
                                    }

                                    let mut sig = (&demangled[start_of_args..]).to_string();

                                    if sig.starts_with("()") {
                                        sig = format!("({}*){}", class_name, &sig[2..]);
                                    } else {
                                        sig = format!("({}*, {}", class_name, &sig[1..]);
                                    }

                                    if sig.ends_with("const") {
                                        sig = sig[0..sig.len() - 5].to_string();
                                        sig = sig.trim_end().to_string();
                                    }

                                    println!("    void (*{}){};", name, sig);
                                }
                            });

                            println!("}};");
                        }

                        println!("struct {} {{", class_name);
                        for field in main_class_fields {
                            println!("    {};", field);
                        }
                        println!("}};");
                    }
                }
                _ => {
                    panic!("unknown action: {:?}", action)
                }
            }
        }
        _ => {}
    }
}
