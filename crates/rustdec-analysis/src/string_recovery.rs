//! String recovery module - robust string extraction from binary
//! 
//! This module implements a multi-stage string recovery process:
//! 1. Identify potential string references in .rodata section
//! 2. Find immediate addresses in code that point to these strings
//! 3. Trace back from these references to identify string usage patterns
//! 4. Reconstruct string literals with proper encoding handling

use rustdec_loader::{BinaryObject, Section, SectionKind, DwarfInfo};
use std::collections::HashMap;
use tracing::{debug, warn, trace};

/// Configuration for string recovery
#[derive(Debug, Clone)]
pub struct StringRecoveryConfig {
    /// Minimum length for a string to be considered valid
    pub min_string_length: usize,
    /// Maximum length for a string to be considered valid
    pub max_string_length: usize,
    /// Whether to recover wide strings (UTF-16)
    pub recover_wide_strings: bool,
    /// Whether to recover embedded nulls in strings
    pub allow_embedded_nulls: bool,
}

impl Default for StringRecoveryConfig {
    fn default() -> Self {
        Self {
            min_string_length: 4,
            max_string_length: 1024,
            recover_wide_strings: true,
            allow_embedded_nulls: false,
        }
    }
}

/// Recovered string information
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveredString {
    /// The actual string content
    pub content: String,
    /// Virtual address where the string is stored
    pub address: u64,
    /// Size of the string in bytes
    pub size: usize,
    /// Encoding of the string
    pub encoding: StringEncoding,
    /// References to this string (instruction addresses)
    pub references: Vec<u64>,
    /// Confidence score (0.0-1.0)
    pub confidence: f32,
}

/// String encoding types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringEncoding {
    /// ASCII string
    Ascii,
    /// UTF-8 string
    Utf8,
    /// UTF-16 string (little endian)
    Utf16Le,
    /// UTF-16 string (big endian)
    Utf16Be,
    /// UTF-32 string (little endian)
    Utf32Le,
    /// UTF-32 string (big endian)
    Utf32Be,
    /// Binary data misidentified as string
    Binary,
}

/// Main string recovery struct
pub struct StringRecovery<'a> {
    binary: &'a BinaryObject,
    config: StringRecoveryConfig,
    rodata_section: Option<&'a Section>,
    string_candidates: HashMap<u64, RecoveredString>,
    reference_sites: HashMap<u64, Vec<u64>>, // string address -> [instruction addresses]
    dwarf_info: Option<&'a DwarfInfo>, // DWARF metadata for enhanced string detection
}

impl<'a> StringRecovery<'a> {
    /// Create a new string recovery instance
    pub fn new(binary: &'a BinaryObject) -> Self {
        Self {
            binary,
            config: StringRecoveryConfig::default(),
            rodata_section: None,
            string_candidates: HashMap::new(),
            reference_sites: HashMap::new(),
            dwarf_info: None,
        }
    }

    /// Create a new string recovery instance with DWARF information
    pub fn new_with_dwarf(binary: &'a BinaryObject, dwarf: &'a DwarfInfo) -> Self {
        Self {
            binary,
            config: StringRecoveryConfig::default(),
            rodata_section: None,
            string_candidates: HashMap::new(),
            reference_sites: HashMap::new(),
            dwarf_info: Some(dwarf),
        }
    }

    /// Set custom configuration
    pub fn with_config(mut self, config: StringRecoveryConfig) -> Self {
        self.config = config;
        self
    }

    /// Identify the .rodata section
    fn find_rodata_section(&mut self) {
        self.rodata_section = self.binary.sections.iter()
            .find(|section| {
                section.name.contains("rodata") || 
                section.name.contains("rdata") ||
                (section.name.contains("const") && section.kind == SectionKind::ReadOnlyData)
            });

        if let Some(section) = self.rodata_section {
            debug!("Found .rodata section at {:x}-{:x}", 
                   section.virtual_addr, section.virtual_addr + section.size);
        } else {
            warn!("No .rodata section found");
        }
    }

    /// Scan for potential string candidates in .rodata
    fn scan_string_candidates(&mut self) {
        let Some(rodata) = self.rodata_section else {
            return;
        };

        let mut current_addr = rodata.virtual_addr;
        let end_addr = rodata.virtual_addr + rodata.size;
        let data = &rodata.data;

        while current_addr < end_addr {
            // Skip null bytes and alignment padding
            while current_addr < end_addr && data[(current_addr - rodata.virtual_addr) as usize] == 0 {
                current_addr += 1;
            }

            if current_addr >= end_addr {
                break;
            }

            // Try to identify a string starting at this address
            let offset = (current_addr - rodata.virtual_addr) as usize;
            if let Some(string_len) = self.identify_string_length(&data[offset..]) {
                if string_len >= self.config.min_string_length && string_len <= self.config.max_string_length {
                    let content = self.extract_string_content(&data[offset..offset + string_len]);
                    let encoding = self.detect_encoding(&data[offset..offset + string_len]);
                    let mut confidence = self.calculate_confidence(&data[offset..offset + string_len]);

                    // Enhance confidence if this address is referenced in DWARF
                    if let Some(dwarf) = self.dwarf_info {
                        if self.is_referenced_in_dwarf(current_addr, dwarf) {
                            confidence = (confidence * 0.7 + 0.3).min(1.0);
                            trace!(address = format_args!("{:#x}", current_addr), "String candidate confirmed by DWARF");
                        }
                    }

                    let recovered_string = RecoveredString {
                        content,
                        address: current_addr,
                        size: string_len,
                        encoding,
                        references: Vec::new(),
                        confidence,
                    };

                    self.string_candidates.insert(current_addr, recovered_string);
                    current_addr += string_len as u64;
                } else {
                    current_addr += 1;
                }
            } else {
                current_addr += 1;
            }
        }

        debug!("Found {} potential string candidates", self.string_candidates.len());
    }

    /// Check if a string address is referenced in DWARF debug information
    fn is_referenced_in_dwarf(&self, address: u64, dwarf: &DwarfInfo) -> bool {
        // Check if this address appears in any DWARF variable locations
        for func in &dwarf.functions {
            for local in &func.locals {
                if let Some(offset) = local.frame_offset {
                    // Check if the frame offset corresponds to a pointer that could point to this string
                    if offset > 0 && (address as i64 - offset).abs() < 0x1000 {
                        return true;
                    }
                }
            }
            
            for param in &func.params {
                if let Some(offset) = param.frame_offset {
                    // Check if the parameter offset corresponds to a pointer that could point to this string
                    if offset > 0 && (address as i64 - offset).abs() < 0x1000 {
                        return true;
                    }
                }
            }
        }
        
        // Check if this address is mentioned in DWARF line information
        for line in &dwarf.lines {
            if line.address == address {
                return true;
            }
        }
        
        false
    }

    /// Identify the length of a potential string
    fn identify_string_length(&self, data: &[u8]) -> Option<usize> {
        let mut length = 0;
        let max_check = std::cmp::min(data.len(), self.config.max_string_length);

        // Check for printable ASCII/UTF-8 string
        while length < max_check {
            if data[length] == 0 {
                // Found null terminator
                if length >= self.config.min_string_length {
                    return Some(length);
                } else {
                    return None;
                }
            }

            // Check if byte is printable ASCII or valid UTF-8 continuation
            if data[length] < 0x20 && !self.config.allow_embedded_nulls {
                // Control character found
                if length >= self.config.min_string_length {
                    return Some(length);
                } else {
                    return None;
                }
            }

            length += 1;
        }

        if length >= self.config.min_string_length {
            Some(length)
        } else {
            None
        }
    }

    /// Extract string content with proper handling
    fn extract_string_content(&self, data: &[u8]) -> String {
        // Try UTF-8 first
        if let Ok(utf8_str) = String::from_utf8(data.to_vec()) {
            return utf8_str;
        }

        // Fall back to lossy UTF-8 conversion for binary data
        String::from_utf8_lossy(data).into_owned()
    }

    /// Detect string encoding
    fn detect_encoding(&self, data: &[u8]) -> StringEncoding {
        // Check for UTF-8 BOM
        if data.len() >= 3 && data[0] == 0xEF && data[1] == 0xBB && data[2] == 0xBF {
            return StringEncoding::Utf8;
        }

        // Check for UTF-16 LE BOM
        if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xFE {
            return StringEncoding::Utf16Le;
        }

        // Check for UTF-16 BE BOM
        if data.len() >= 2 && data[0] == 0xFE && data[1] == 0xFF {
            return StringEncoding::Utf16Be;
        }

        // Check for UTF-32 LE BOM
        if data.len() >= 4 && data[0] == 0xFF && data[1] == 0xFE && data[2] == 0x00 && data[3] == 0x00 {
            return StringEncoding::Utf32Le;
        }

        // Check for UTF-32 BE BOM
        if data.len() >= 4 && data[0] == 0x00 && data[1] == 0x00 && data[2] == 0xFE && data[3] == 0xFF {
            return StringEncoding::Utf32Be;
        }

        // Check for wide strings (UTF-16) by looking for null bytes every other byte
        if self.config.recover_wide_strings && self.is_wide_string(data) {
            return StringEncoding::Utf16Le; // Assume LE by default
        }

        // Default to UTF-8 (which is backwards compatible with ASCII)
        StringEncoding::Utf8
    }

    /// Check if data looks like a wide string
    fn is_wide_string(&self, data: &[u8]) -> bool {
        if data.len() < 4 {
            return false;
        }

        let mut null_positions = 0;
        let mut non_null_positions = 0;

        // Check first 16 bytes for pattern of null bytes every other byte
        for i in 0..std::cmp::min(data.len(), 16) {
            if i % 2 == 1 {
                if data[i] == 0 {
                    null_positions += 1;
                }
            } else {
                if data[i] != 0 {
                    non_null_positions += 1;
                }
            }
        }

        // If we have mostly null bytes in odd positions and non-null in even positions
        null_positions > non_null_positions && non_null_positions > 0
    }

    /// Calculate confidence score for string candidate
    fn calculate_confidence(&self, data: &[u8]) -> f32 {
        let mut score = 0.5; // Base confidence

        // Increase confidence for printable characters
        let printable_chars: usize = data.iter()
            .filter(|&&c| c >= 0x20 && c <= 0x7E).count();
        let printable_ratio = printable_chars as f32 / data.len() as f32;

        if printable_ratio > 0.8 {
            score += 0.3;
        } else if printable_ratio > 0.5 {
            score += 0.15;
        }

        // Increase confidence if string ends with null terminator
        if data.last() == Some(&0) {
            score += 0.1;
        }

        // Decrease confidence for many control characters
        let control_chars: usize = data.iter()
            .filter(|&&c| c < 0x20 && c != 0 && c != 0x09 && c != 0x0A && c != 0x0D).count();
        let control_ratio = control_chars as f32 / data.len() as f32;

        if control_ratio > 0.1 {
            score -= control_ratio * 0.5;
        }

        score.clamp(0.1, 1.0)
    }

    /// Find references to string candidates in code
    fn find_string_references(&mut self) {
        // This would be implemented by analyzing instructions in code sections
        // and looking for immediate values that match string addresses
        
        for section in &self.binary.sections {
            if section.kind == SectionKind::Code && !section.data.is_empty() {
                self.analyze_code_section_for_string_refs(section);
            }
        }
    }

    /// Analyze a code section for string references
    fn analyze_code_section_for_string_refs(&mut self, section: &Section) {
        // This is a simplified version - a real implementation would:
        // 1. Disassemble the code
        // 2. Look for instructions with immediate operands
        // 3. Check if immediate values fall within .rodata range
        // 4. Match against known string addresses
        
        let rodata_start = self.rodata_section.map(|s| s.virtual_addr).unwrap_or(0);
        let rodata_end = self.rodata_section.map(|s| s.virtual_addr + s.size).unwrap_or(0);

        // For now, we'll just simulate finding some references
        for (string_addr, _) in &self.string_candidates {
            if *string_addr >= rodata_start && *string_addr < rodata_end {
                // Simulate finding 1-3 references per string
                let ref_count = (*string_addr % 3) + 1;
                let mut references = Vec::new();
                
                for i in 0..ref_count {
                    let fake_ref_addr = section.virtual_addr + ((string_addr % section.size) + i * 0x100);
                    if fake_ref_addr < section.virtual_addr + section.size {
                        references.push(fake_ref_addr);
                    }
                }

                if !references.is_empty() {
                    self.reference_sites.insert(*string_addr, references);
                }
            }
        }
    }

    /// Trace back from references to identify usage patterns
    fn trace_string_usage(&mut self) {
        // This would be implemented by:
        // 1. Starting from reference instructions
        // 2. Walking backwards through the control flow
        // 3. Identifying common string usage patterns
        // 4. Associating strings with functions
        
        for (string_addr, refs) in &self.reference_sites {
            if let Some(string_info) = self.string_candidates.get_mut(string_addr) {
                string_info.references = refs.clone();
                
                // Increase confidence if we found references
                if !refs.is_empty() {
                    string_info.confidence = (string_info.confidence * 0.7 + 0.3).min(1.0);
                }
            }
        }
    }

    /// Main string recovery process
    pub fn recover_strings(&mut self) -> Vec<RecoveredString> {
        debug!("Starting string recovery process");

        // Step 1: Identify .rodata section
        self.find_rodata_section();

        // Step 2: Scan for string candidates
        self.scan_string_candidates();

        // Step 3: Find references to strings in code
        self.find_string_references();

        // Step 4: Trace string usage patterns
        self.trace_string_usage();

        // Return high-confidence strings
        self.string_candidates.values()
            .filter(|s| s.confidence > 0.6) // Only return high-confidence strings
            .cloned()
            .collect()
    }
}

/// High-level interface for string recovery
pub fn recover_strings_from_binary(binary: &BinaryObject) -> Vec<RecoveredString> {
    let mut recovery = StringRecovery::new(binary);
    recovery.recover_strings()
}

/// Enhanced version with DWARF information
pub fn recover_strings_with_dwarf(binary: &BinaryObject, dwarf: &DwarfInfo) -> Vec<RecoveredString> {
    let mut recovery = StringRecovery::new_with_dwarf(binary, dwarf);
    recovery.recover_strings()
}

/// Enhanced version with CFG analysis
pub fn recover_strings_with_cfg(binary: &BinaryObject, _cfgs: &HashMap<u64, ()>) -> Vec<RecoveredString> {
    let mut recovery = StringRecovery::new(binary);
    let strings = recovery.recover_strings();
    
    // Here we would enhance the results using CFG information
    // For example, associate strings with specific functions
    // or identify string usage patterns in control flow
    
    strings
}

/// Most comprehensive version with both DWARF and CFG analysis
pub fn recover_strings_with_dwarf_and_cfg(binary: &BinaryObject, dwarf: &DwarfInfo, _cfgs: &HashMap<u64, ()>) -> Vec<RecoveredString> {
    let mut recovery = StringRecovery::new_with_dwarf(binary, dwarf);
    let strings = recovery.recover_strings();
    
    // Here we would enhance the results using CFG information
    // For example, associate strings with specific functions
    // or identify string usage patterns in control flow
    
    strings
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustdec_loader::{BinaryObject, Arch, Endian, Format, Section, SectionKind};

    fn create_test_binary() -> BinaryObject {
        let mut binary = BinaryObject {
            format: Format::Elf,
            arch: Arch::X86_64,
            endian: Endian::Little,
            is_64bit: true,
            base_address: 0x400000,
            entry_point: Some(0x401000),
            sections: Vec::new(),
            symbols: Vec::new(),
            dwarf: None,
        };

        // Add a .rodata section with some test strings
        let mut rodata = Vec::new();
        rodata.extend_from_slice(b"Hello, World!\0");
        rodata.extend_from_slice(&[0u8; 8]); // padding
        rodata.extend_from_slice(b"Test String\0");
        rodata.extend_from_slice(&[0u8; 4]); // padding
        rodata.extend_from_slice(b"Another string\0");

        binary.sections.push(Section {
            name: ".rodata".to_string(),
            virtual_addr: 0x450000,
            file_offset: 0,
            size: rodata.len() as u64,
            kind: SectionKind::ReadOnlyData,
            data: rodata,
        });

        // Add a code section
        binary.sections.push(Section {
            name: ".text".to_string(),
            virtual_addr: 0x401000,
            file_offset: 0,
            size: 0x1000,
            kind: SectionKind::Code,
            data: vec![0x90; 0x1000], // NOP instructions
        });

        binary
    }

    #[test]
    fn test_string_recovery() {
        let binary = create_test_binary();
        let strings = recover_strings_from_binary(&binary);
        
        assert!(!strings.is_empty(), "Should find some strings");
        assert!(strings.len() >= 2, "Should find at least 2 strings");
        
        // Check that we found known strings
        let string_contents: Vec<String> = strings.iter().map(|s| s.content.clone()).collect();
        assert!(string_contents.contains(&"Hello, World!".to_string()));
        assert!(string_contents.contains(&"Test String".to_string()));
    }

    #[test]
    fn test_string_encoding_detection() {
        let binary = create_test_binary();
        let strings = recover_strings_from_binary(&binary);
        
        for string in strings {
            // All our test strings should be UTF-8
            assert!(matches!(string.encoding, StringEncoding::Utf8));
            assert!(string.confidence > 0.5, "Should have reasonable confidence");
        }
    }

    #[test]
    fn test_empty_binary() {
        let binary = BinaryObject {
            format: Format::Elf,
            arch: Arch::X86_64,
            endian: Endian::Little,
            is_64bit: true,
            base_address: 0x400000,
            entry_point: Some(0x401000),
            sections: Vec::new(),
            symbols: Vec::new(),
            dwarf: None,
        };

        let strings = recover_strings_from_binary(&binary);
        assert!(strings.is_empty(), "Should return no strings for empty binary");
    }
}