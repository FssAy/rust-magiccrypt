extern crate base64;

extern crate digest;
extern crate digest_old;

extern crate block_cipher_trait;
extern crate block_modes;

extern crate aes_soft as aes;

extern crate md5;

use std::intrinsics::copy;
use std::io::{ErrorKind, Read, Write};

use crate::functions::*;
use crate::{MagicCryptError, MagicCryptTrait, BUFFER_SIZE};

use digest::generic_array::GenericArray;
use digest::Digest;
use digest_old::FixedOutput as OldFixedOutput;

use block_cipher_trait::BlockCipher;
use block_modes::block_padding::{Padding, Pkcs7};
use block_modes::{BlockMode, Cbc};

use aes::Aes192;

use md5::Md5;
use tiger_digest::Tiger;

type Aes192Cbc = Cbc<Aes192, Pkcs7>;
type Key = GenericArray<u8, <Aes192 as BlockCipher>::KeySize>;
type Block = GenericArray<u8, <Aes192 as BlockCipher>::BlockSize>;

const BLOCK_SIZE: usize = 16;

/// This struct can help you encrypt or decrypt data via AES-192 in a quick way.
#[derive(Debug, Clone)]
pub struct MagicCrypt192 {
    key: Key,
    iv: Block,
}

impl MagicCryptTrait for MagicCrypt192 {
    fn new<S: AsRef<str>, V: AsRef<str>>(key: S, iv: Option<V>) -> MagicCrypt192 {
        let iv = match iv {
            Some(s) => {
                let mut hasher = Md5::new();
                hasher.input(s.as_ref().as_bytes());

                hasher.result()
            }
            None => GenericArray::default(),
        };

        let key = {
            let mut hasher = Tiger::default();
            hasher.consume(key.as_ref().as_bytes());

            GenericArray::clone_from_slice(&hasher.fixed_result().as_slice())
        };

        MagicCrypt192 {
            key,
            iv,
        }
    }

    fn encrypt_to_bytes<T: ?Sized + AsRef<[u8]>>(&self, data: &T) -> Vec<u8> {
        let data = data.as_ref();

        let data_length = data.len();

        let final_length = get_aes_cipher_len(data_length);

        let mut final_result = data.to_vec();

        final_result.reserve_exact(final_length - data_length);

        unsafe {
            final_result.set_len(final_length);
        }

        let cipher = Aes192Cbc::new_fix(&self.key, &self.iv);

        cipher.encrypt(&mut final_result, data_length).unwrap();

        final_result
    }

    fn encrypt_reader_to_bytes(&self, reader: &mut dyn Read) -> Result<Vec<u8>, MagicCryptError> {
        let mut data = Vec::new();

        reader.read_to_end(&mut data)?;

        let data_length = data.len();

        let final_length = get_aes_cipher_len(data_length);

        let mut final_result = data.to_vec();

        final_result.reserve_exact(final_length - data_length);

        unsafe {
            final_result.set_len(final_length);
        }

        let cipher = Aes192Cbc::new_fix(&self.key, &self.iv);

        cipher.encrypt(&mut final_result, data_length).unwrap();

        Ok(final_result)
    }

    fn encrypt_reader_to_writer(
        &self,
        reader: &mut dyn Read,
        writer: &mut dyn Write,
    ) -> Result<(), MagicCryptError> {
        let mut cipher = Aes192Cbc::new_fix(&self.key, &self.iv);

        let mut buffer = [0u8; BUFFER_SIZE];

        let mut l = 0;

        loop {
            match reader.read(&mut buffer[l..]) {
                Ok(c) => {
                    if c == 0 {
                        break;
                    }

                    l += c;

                    if l < BLOCK_SIZE {
                        continue;
                    }

                    let r = l % BLOCK_SIZE;
                    let e = l - r;

                    cipher.encrypt_blocks(to_blocks(&mut buffer[..e]));

                    writer.write_all(&buffer[..e])?;

                    unsafe {
                        copy(buffer.as_ptr().add(e), buffer.as_mut_ptr(), r);
                    }

                    l = r;
                }
                Err(ref err) if err.kind() == ErrorKind::Interrupted => {}
                Err(err) => return Err(MagicCryptError::IOError(err)),
            }
        }

        cipher.encrypt_blocks(to_blocks(Pkcs7::pad(&mut buffer, l, BLOCK_SIZE).unwrap()));

        writer.write_all(&buffer[..get_aes_cipher_len(l)])?;

        Ok(writer.flush()?)
    }

    fn decrypt_bytes_to_bytes<T: ?Sized + AsRef<[u8]>>(
        &self,
        bytes: &T,
    ) -> Result<Vec<u8>, MagicCryptError> {
        let bytes = bytes.as_ref();

        let mut final_result = bytes.to_vec();

        let cipher = Aes192Cbc::new_fix(&self.key, &self.iv);

        let length = cipher.decrypt(&mut final_result)?.len();

        unsafe {
            final_result.set_len(length);
        }

        Ok(final_result)
    }

    fn decrypt_reader_to_bytes(&self, reader: &mut dyn Read) -> Result<Vec<u8>, MagicCryptError> {
        let mut bytes = Vec::new();

        reader.read_to_end(&mut bytes)?;

        let mut final_result = bytes.to_vec();

        let cipher = Aes192Cbc::new_fix(&self.key, &self.iv);

        let length = cipher.decrypt(&mut final_result)?.len();

        unsafe {
            final_result.set_len(length);
        }

        Ok(final_result)
    }

    #[allow(clippy::many_single_char_names)]
    fn decrypt_reader_to_writer(
        &self,
        reader: &mut dyn Read,
        writer: &mut dyn Write,
    ) -> Result<(), MagicCryptError> {
        let mut cipher = Aes192Cbc::new_fix(&self.key, &self.iv);

        let mut buffer = [0u8; BUFFER_SIZE + 1];

        let mut l = 0;

        loop {
            match reader.read(&mut buffer[l..BUFFER_SIZE]) {
                Ok(c) => {
                    l += c;

                    if c > 0 && l < BLOCK_SIZE {
                        continue;
                    }

                    let r = l % BLOCK_SIZE;
                    let e = if r > 0 {
                        l + BLOCK_SIZE - r
                    } else {
                        l
                    };

                    reader.read_exact(&mut buffer[l..e])?;

                    match reader.read_exact(&mut buffer[e..(e + 1)]) {
                        Ok(()) => {
                            cipher.decrypt_blocks(to_blocks(&mut buffer[..e]));

                            writer.write_all(&buffer[..e])?;

                            buffer[0] = buffer[e];

                            l = 1;
                        }
                        Err(ref err) if err.kind() == ErrorKind::UnexpectedEof => {
                            cipher.decrypt_blocks(to_blocks(&mut buffer[..e]));

                            writer.write_all(Pkcs7::unpad(&buffer[..e]).unwrap())?;

                            break;
                        }
                        Err(err) => return Err(MagicCryptError::IOError(err)),
                    }
                }
                Err(ref err) if err.kind() == ErrorKind::Interrupted => {}
                Err(err) => return Err(MagicCryptError::IOError(err)),
            }
        }

        Ok(writer.flush()?)
    }
}