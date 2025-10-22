use std::io::{Cursor, Read, Seek};

pub struct BinReader<'a> {
    cursor: Cursor<&'a Vec<u8>>,
    data: &'a Vec<u8>,
}

impl<'a> BinReader<'a> {
    pub fn new(data: &'a Vec<u8>) -> Self {
        Self {
            cursor: Cursor::new(data),
            data,
        }
    }

    pub fn get_position(&self) -> u64 {
        self.cursor.position()
    }

    pub fn set_position(&mut self, offset: u32) {
        self.cursor.set_position(offset as u64);
    }

    pub fn set_position_relative(&mut self, offset: i32) {
        self.cursor
            .set_position((self.cursor.position() as i64 + offset as i64) as u64);
    }

    pub fn read_u32(&mut self) -> Option<u32> {
        let mut buffer = [0u8; 4];
        match self.cursor.read(&mut buffer) {
            Ok(size) => {
                if size == 4 {
                    Some(u32::from_le_bytes(buffer))
                } else {
                    None
                }
            }
            Err(_) => None,
        }
    }

    pub fn read_i32(&mut self) -> Option<i32> {
        let mut buffer = [0u8; 4];
        match self.cursor.read(&mut buffer) {
            Ok(size) => {
                if size == 4 {
                    Some(i32::from_le_bytes(buffer))
                } else {
                    None
                }
            }
            Err(_) => None,
        }
    }

    pub fn read_u8(&mut self) -> Option<u8> {
        let mut buffer = [0u8; 1];
        match self.cursor.read(&mut buffer) {
            Ok(size) => {
                if size == 1 {
                    Some(buffer[0])
                } else {
                    None
                }
            }
            Err(_) => None,
        }
    }

    pub fn read_cstr(&mut self, adjust: Option<impl Fn(u32) -> u32>) -> Option<String> {
        let mut buffer: Vec<u8> = vec![];

        let return_offset = self.get_position() + 4;

        let mut position_to_string = match self.read_u32() {
            Some(offset) => offset,
            None => return None,
        };

        if let Some(adjust_fn) = adjust {
            position_to_string = adjust_fn(position_to_string);
        }

        self.set_position(position_to_string);

        while let Some(byte) = self.read_u8() {
            if byte == 0 {
                break;
            }
            buffer.push(byte);
        }

        self.set_position(return_offset as u32);

        match String::from_utf8(buffer) {
            Ok(str) => Some(str),
            Err(_) => None,
        }
    }

    pub fn get_data(&self) -> &'a Vec<u8> {
        self.data
    }
}
