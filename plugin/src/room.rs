//! PaperConnect room code format shared with the BMCBL implementation.

const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKLMNPQRSTUVWXYZ";

pub fn generate_room() -> Result<String, getrandom::Error> {
    Ok(format!("P/{}-{}", random_group()?, random_group()?))
}

pub fn room_from_code(room_code: &str) -> Result<String, String> {
    let raw = room_code
        .trim()
        .strip_prefix("P/")
        .ok_or("房间码应以 P/ 开头。")?;
    let parts = raw.split('-').collect::<Vec<_>>();
    if parts.len() != 4
        || parts.iter().any(|part| {
            part.len() != 4
                || !part
                    .bytes()
                    .all(|byte| ALPHABET.contains(&byte.to_ascii_uppercase()))
        })
    {
        return Err("房间码格式应为 P/NNNN-NNNN-SSSS-SSSS。".into());
    }
    // Older implementations published valid-format codes without the generation checksum.
    Ok(format!("P/{}", raw.to_ascii_uppercase()))
}

fn random_group() -> Result<String, getrandom::Error> {
    loop {
        let mut raw = [0_u8; 16];
        getrandom::fill(&mut raw)?;
        let mut digits = [0; 8];
        let mut count = 0;
        for byte in raw
            .into_iter()
            .filter(|byte| *byte < 238)
            .take(digits.len())
        {
            digits[count] = byte % 34;
            count += 1;
        }
        if count != digits.len() {
            continue;
        }
        let checksum = digits
            .iter()
            .rev()
            .fold(0_u32, |value, digit| (value * 34 + u32::from(*digit)) % 7);
        if checksum != 0 {
            continue;
        }
        let mut formatted = String::with_capacity(9);
        for (index, digit) in digits.into_iter().enumerate() {
            if index == 4 {
                formatted.push('-');
            }
            formatted.push(ALPHABET[usize::from(digit)].into());
        }
        return Ok(formatted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_generated_and_legacy_codes() {
        let generated = generate_room().unwrap();
        assert_eq!(room_from_code(&generated).unwrap(), generated);
        assert_eq!(
            room_from_code("P/abcd-abcd-1234-1234").unwrap(),
            "P/ABCD-ABCD-1234-1234"
        );
        assert!(room_from_code("P/abci-abcd-1234-1234").is_err());
    }
}
