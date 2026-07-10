use std::fs;
use koc_core::KocCore;

fn main() {
    let bin_path = "liulian.bin";
    let bin_data = fs::read(bin_path).expect("Failed to read liulian.bin");
    println!("Read {} bytes from {}", bin_data.len(), bin_path);
    println!(
        "Header: [{}, {}] => {}",
        bin_data[0],
        bin_data[1],
        if bin_data[0] == 112 && bin_data[1] == 108 {
            "LZ4 encrypted"
        } else if bin_data[0] == 112 && bin_data[1] == 120 {
            "XOR encrypted"
        } else {
            "unknown"
        }
    );

    // 1. get token id (md5)
    let token_id = KocCore::get_token_id(&bin_data);
    println!("\nToken ID (md5): {}", token_id);

    // 2. parse bin content
    let core = KocCore::new();
    match core.parse_bin(&bin_data) {
        Ok(data) => {
            println!("\nParsed bin data ({} fields):", data.len());
            for (k, v) in &data {
                let val_str = format!("{}", v);
                if val_str.len() > 100 {
                    println!("  {} = {}...", k, &val_str[..100]);
                } else {
                    println!("  {} = {}", k, val_str);
                }
            }
        }
        Err(e) => {
            println!("\nFailed to parse bin: {}", e);
            // try raw auto_decrypt to see what we get
            let decrypted = koc_core::auto_decrypt(&bin_data);
            println!(
                "Decrypted {} bytes, first 50: {:?}",
                decrypted.len(),
                &decrypted[..50.min(decrypted.len())]
            );
        }
    }
}
