//! Queries compose Blobray operations and format their typed results.
use super::*;
use app::library::{FunctionResult, LibraryOutcome};
use oer_analysis_riscv_model::AbstractValue;

fn mode(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Load | MemoryKind::LoadReserved | MemoryKind::FloatLoad => "R",
        MemoryKind::Atomic => "RW",
        _ => "W",
    }
}

fn observation_line(
    corpus: &Corpus,
    metadata: &Metadata,
    record: &RegisterAccess,
    location: &str,
) -> Result<String> {
    let RegisterAccess::Observation {
        function,
        address,
        fact,
        stored,
        ..
    } = record
    else {
        return Err(invalid("only a memory observation has an access line"));
    };
    let FunctionRecord::MemoryAccess {
        offset,
        access: kind,
        width,
        ..
    } = fact.as_ref()
    else {
        return Err(invalid("memory observation has no memory fact"));
    };
    let function = metadata.function(function)?;
    let offset = offset
        .checked_sub(function.entry)
        .ok_or_else(|| invalid("access precedes its function"))?;
    let width_note = if *width == 4 {
        String::new()
    } else {
        format!(" {width}-byte")
    };
    let detail = if mode(*kind) == "R" {
        String::new()
    } else {
        format::effect(corpus, *address, *width, stored.as_deref())
    };
    Ok(format!(
        "{location} {}{width_note} {}::{}+{offset:#x}  {detail}",
        mode(*kind),
        function.origin,
        function.name
    )
    .trim_end()
    .to_owned())
}

pub fn xref(corpus: &Corpus, start: u32, end: u32) -> Result<String> {
    if start >= end {
        return Ok(String::new());
    }
    let mut budget = Budget::new()?;
    let metadata = corpus.metadata(None, &mut budget)?;
    let mut lines = BTreeSet::new();
    let (_, limited) = app::library::register_accesses(
        &corpus.inputs,
        assumptions(&[]),
        &[ImageRegion {
            start,
            length: u64::from(end - start),
        }],
        &RiscvDecoder,
        &budget.memory,
        &mut budget.control,
        &mut |record, _| {
            if let RegisterAccess::Observation {
                function,
                address: Some(address),
                fact,
                ..
            } = record
                && (start..end).contains(address)
                && let FunctionRecord::MemoryAccess { offset, .. } = fact.as_ref()
            {
                let line =
                    observation_line(corpus, &metadata, record, &corpus.registers.name(*address))
                        .map_err(domain_error)?;
                let info = metadata.function(function).map_err(domain_error)?;
                lines.insert((
                    *address,
                    info.origin.clone(),
                    info.name.clone(),
                    *offset,
                    line,
                ));
            }
            Ok(())
        },
    )?;
    budget.report("register-accesses", limited)?;
    Ok(lines
        .into_iter()
        .map(|(.., line)| format!("{line}\n"))
        .collect())
}

fn domain_error(
    error: Box<dyn std::error::Error + Send + Sync>,
) -> oer_analysis_riscv_model::Error {
    oer_analysis_riscv_model::Error::new(
        oer_analysis_riscv_model::ErrorCode::Integrity,
        error.to_string(),
    )
}

pub fn show(corpus: &Corpus, name: &str) -> Result<Option<String>> {
    let names = [name.to_owned()];
    let mut budget = Budget::new()?;
    let metadata = corpus.metadata(Some(&names), &mut budget)?;
    let mut request = assumptions(&names);
    request.symbolic_stores = true;
    let mut notes: BTreeMap<(LibraryFunction, u64), Vec<String>> = BTreeMap::new();
    let (_, limited) = app::library::register_accesses(
        &corpus.inputs,
        request,
        &[],
        &RiscvDecoder,
        &budget.memory,
        &mut budget.control,
        &mut |record, _| {
            if let RegisterAccess::Observation {
                function,
                address,
                fact,
                stored,
                reads,
                ..
            } = record
                && let FunctionRecord::MemoryAccess {
                    offset,
                    access,
                    width,
                    address: value,
                    ..
                } = fact.as_ref()
            {
                let location = address.map_or_else(
                    || format::value(value, &metadata),
                    |a| corpus.registers.name(a),
                );
                let mut note = format!("[{location}]");
                if mode(*access) != "R" {
                    let _ = write!(
                        note,
                        " {}",
                        format::effect(corpus, *address, *width, stored.as_deref())
                    );
                } else if let Some(reads) = reads {
                    let _ = write!(note, " {}", format::read_bits(corpus, *address, reads));
                }
                notes
                    .entry((function.clone(), *offset))
                    .or_default()
                    .push(note);
            }
            Ok(())
        },
    )?;
    budget.report("register-accesses", limited)?;
    budget = Budget::new()?;
    let mut definitions = BTreeMap::new();
    let records_limited = app::library::analyze_library(
        &corpus.inputs,
        assumptions(&names),
        &RiscvDecoder,
        &budget.memory,
        &mut budget.control,
        &mut |outcome, _| {
            match outcome {
                LibraryOutcome::Analyzed(analyzed) => {
                    let function = metadata.function(analyzed.function).map_err(domain_error)?;
                    let mut text = format!(
                        "== {}::{} at {:#x}, {:#x} bytes\n",
                        function.origin, function.name, function.entry, function.size
                    );
                    let mut extra = BTreeMap::<u64, Vec<String>>::new();
                    for record in analyzed.records {
                        match record {
                            FunctionRecord::Value {
                                offset,
                                register,
                                value,
                                ..
                            } if ![0, 1, 2].contains(register) => {
                                if !matches!(value, AbstractValue::Unknown) {
                                    extra.entry(*offset).or_default().push(format!(
                                        "{} = {}",
                                        oer_analysis_riscv_lift::register_name(*register),
                                        format::value(value, &metadata)
                                    ));
                                }
                            }
                            FunctionRecord::Reference {
                                raw,
                                target,
                                addend,
                                ..
                            } => {
                                let name = String::from_utf8_lossy(&raw.target.name);
                                let mut note = name.into_owned();
                                if let Some(addend) = raw.addend.filter(|a| *a != 0) {
                                    let _ = write!(note, "{addend:+#x}");
                                }
                                if let Some(literal) =
                                    metadata.literal(&target.symbol, addend.unwrap_or(0))
                                {
                                    let _ = write!(note, " \"{literal}\"");
                                }
                                if !note.is_empty() {
                                    extra.entry(raw.offset).or_default().push(note);
                                }
                            }
                            FunctionRecord::Transfer {
                                offset,
                                target: AbstractValue::ImageAddress { address },
                                ..
                            } => {
                                let target = metadata
                                    .names
                                    .get(&u64::from(*address))
                                    .cloned()
                                    .unwrap_or_else(|| format!("{address:#x}"));
                                extra
                                    .entry(*offset)
                                    .or_default()
                                    .push(format!("<{target}>"));
                            }
                            _ => {}
                        }
                    }
                    for record in analyzed.records {
                        if let FunctionRecord::Instruction {
                            offset, decoded, ..
                        } = record
                        {
                            let mut annotations = extra.remove(offset).unwrap_or_default();
                            if let Some(accesses) = notes.get(&(analyzed.function.clone(), *offset))
                            {
                                annotations.extend(accesses.iter().cloned());
                            }
                            let offset = offset.checked_sub(function.entry).ok_or_else(|| {
                                domain_error(invalid("instruction precedes its function"))
                            })?;
                            let _ = write!(text, "  +{offset:04x}  {}", decoded.text);
                            if !annotations.is_empty() {
                                let _ = write!(text, "    ; {}", annotations.join(", "));
                            }
                            text.push('\n');
                        }
                    }
                    let displayed = analyzed
                        .records
                        .iter()
                        .flat_map(|record| match record {
                            FunctionRecord::Value {
                                register, value, ..
                            } if ![0, 1, 2].contains(register) => [Some(value), None],
                            FunctionRecord::MemoryAccess { address, value, .. } => {
                                [Some(address), value.as_ref()]
                            }
                            _ => [None, None],
                        })
                        .flatten();
                    text.push_str(&format::dictionary(analyzed.records, displayed, &metadata));
                    definitions.insert(analyzed.function.clone(), text);
                }
                LibraryOutcome::Blocked { function, error } => {
                    let function = metadata.function(function).map_err(domain_error)?;
                    return Err(oer_analysis_riscv_model::Error::new(
                        error.code,
                        format!("{}::{}: {}", function.origin, function.name, error.message),
                    ));
                }
                LibraryOutcome::Gap { .. } => {}
            }
            Ok(())
        },
    )?;
    budget.report("function-records", records_limited)?;
    let text: String = definitions.into_values().collect();
    Ok((!text.is_empty()).then_some(text))
}

pub fn fields(corpus: &Corpus, offsets: &[i32]) -> Result<String> {
    let Some(last) = offsets.last() else {
        return Err(invalid("field offsets are empty"));
    };
    let mut budget = Budget::new()?;
    let metadata = corpus.metadata(None, &mut budget)?;
    let wanted: Vec<_> = offsets.iter().copied().map(i64::from).collect();
    let mut selected = BTreeMap::new();
    let summary = app::library::field_accesses(
        &corpus.inputs,
        assumptions(&[]),
        i64::from(*last),
        None,
        &RiscvDecoder,
        &budget.memory,
        &mut budget.control,
        &mut |result, _| {
            if let FunctionResult::Matched(function) = result {
                for field in &function.accesses {
                    if field.path.ends_with(&wanted) {
                        let mut location = match &field.root {
                            app::field::FieldRoot::EntryRegister { register } => {
                                oer_analysis_riscv_lift::register_name(*register).to_owned()
                            }
                            app::field::FieldRoot::Symbol { name, symbol } => name
                                .as_ref()
                                .map(|n| format!("&{n}"))
                                .unwrap_or_else(|| format!("&symbol{}", symbol.index)),
                            root => format!("{root:?}"),
                        };
                        for offset in &field.path {
                            let _ = write!(location, "->{offset:#x}");
                        }
                        selected.insert((function.function.clone(), field.offset), location);
                    }
                }
            }
            Ok(())
        },
    )?;
    let names: Vec<_> = selected
        .keys()
        .filter_map(|(f, _)| f.name.as_deref())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    budget.report("field-accesses", summary.analysis.limited)?;
    if names.is_empty() {
        return Ok(String::new());
    }
    budget = Budget::new()?;
    let mut lines = BTreeSet::new();
    let mut request = assumptions(&names);
    request.symbolic_stores = true;
    let (_, limited) = app::library::register_accesses(
        &corpus.inputs,
        request,
        &[],
        &RiscvDecoder,
        &budget.memory,
        &mut budget.control,
        &mut |record, _| {
            if let RegisterAccess::Observation { function, fact, .. } = record
                && let FunctionRecord::MemoryAccess { offset, .. } = fact.as_ref()
                && let Some(location) = selected.get(&(function.clone(), *offset))
            {
                lines.insert(
                    observation_line(corpus, &metadata, record, location).map_err(domain_error)?,
                );
            }
            Ok(())
        },
    )?;
    budget.report("register-accesses", limited)?;
    Ok(lines.into_iter().map(|line| format!("{line}\n")).collect())
}

pub fn prints(corpus: &Corpus, start: u32, end: u32) -> Result<String> {
    if start >= end {
        return Ok(String::new());
    }
    let mut budget = Budget::new()?;
    let metadata = corpus.metadata(Some(&[]), &mut budget)?;
    let mut calls = BTreeMap::new();
    let mut callees = BTreeSet::new();
    let mut dictionaries = BTreeMap::new();
    let limited = app::library::analyze_library(
        &corpus.inputs,
        assumptions(&[]),
        &RiscvDecoder,
        &budget.memory,
        &mut budget.control,
        &mut |outcome, _| {
            if let LibraryOutcome::Analyzed(a) = outcome {
                for record in a.records {
                    if let FunctionRecord::CallInputs { offset, registers } = record {
                        for register in 10..18 {
                            let Some(literal) = registers
                                .get(register)
                                .and_then(|v| metadata.literal_value(&a.function.symbol.object, v))
                            else {
                                continue;
                            };
                            let argument_count = conversions(&literal).len();
                            if argument_count == 0 {
                                continue;
                            }
                            calls.insert(
                                (a.function.clone(), *offset),
                                (register, literal, registers.clone()),
                            );
                            for record in a.records {
                                if let FunctionRecord::Reference {
                                    reference_kind: oer_analysis_riscv_model::ReferenceKind::Call,
                                    target,
                                    ..
                                } = record
                                    && !target.name.is_empty()
                                {
                                    callees
                                        .insert(String::from_utf8_lossy(&target.name).into_owned());
                                }
                                if let FunctionRecord::Transfer {
                                    offset: site,
                                    target,
                                    ..
                                } = record
                                    && site == offset
                                {
                                    let name = match target {
                                        AbstractValue::Symbol { symbol, .. } => metadata
                                            .symbols
                                            .get(symbol)
                                            .and_then(|s| s.name.as_deref())
                                            .map(|n| String::from_utf8_lossy(n).into_owned()),
                                        AbstractValue::ImageAddress { address }
                                        | AbstractValue::Constant { value: address } => {
                                            metadata.names.get(&u64::from(*address)).cloned()
                                        }
                                        _ => None,
                                    };
                                    if let Some(name) = name {
                                        callees.insert(name);
                                    }
                                }
                            }
                            let arguments =
                                registers.iter().skip(register + 1).take(argument_count);
                            dictionaries.insert(
                                a.function.clone(),
                                format::dictionary(a.records, arguments, &metadata),
                            );
                            break;
                        }
                    }
                }
            }
            Ok(())
        },
    )?;
    let names: Vec<_> = calls
        .keys()
        .filter_map(|(f, _)| f.name.as_deref())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    budget.report("function-records", limited)?;
    if names.is_empty() {
        return Ok(String::new());
    }
    budget = Budget::new()?;
    let callees: Vec<_> = callees.into_iter().collect();
    let mut request = assumptions(&names);
    request.call_results = &callees;
    let mut lines = BTreeSet::new();
    let mut symbolic = BTreeSet::new();
    let (_, read_limits) = app::library::register_accesses(
        &corpus.inputs,
        request,
        &[ImageRegion {
            start,
            length: u64::from(end - start),
        }],
        &RiscvDecoder,
        &budget.memory,
        &mut budget.control,
        &mut |record, _| {
            if let RegisterAccess::Observation {
                function,
                address: Some(address),
                reads: Some(reads),
                ..
            } = record
                && (start..end).contains(address)
            {
                for usage in &reads.uses {
                    let oer_analysis_blobray_domain::ReadSink::CallArgument {
                        site,
                        register,
                        shift,
                        ..
                    } = &usage.sink
                    else {
                        continue;
                    };
                    let Some((format_register, literal, registers)) =
                        calls.get(&(function.clone(), *site))
                    else {
                        continue;
                    };
                    let Some(argument) = usize::from(*register).checked_sub(format_register + 1)
                    else {
                        continue;
                    };
                    let Some(conversion) = conversions(literal).get(argument).cloned() else {
                        continue;
                    };
                    let wiring = match shift {
                        Some(shift) if *shift <= 0 => format!(">> {}", -i16::from(*shift)),
                        Some(shift) => format!("<< {shift}"),
                        None => {
                            symbolic.insert(function.clone());
                            format!(
                                "=> {}",
                                registers
                                    .get(usize::from(*register))
                                    .map(|v| format::value(v, &metadata))
                                    .unwrap_or_else(|| "?".into())
                            )
                        }
                    };
                    let info = metadata.function(function).map_err(domain_error)?;
                    let at = site
                        .checked_sub(info.entry)
                        .ok_or_else(|| domain_error(invalid("print precedes its function")))?;
                    lines.insert(format!(
                        "{} {} {wiring} -> \"{conversion}\" {}::{}+{at:#x}",
                        corpus.registers.name(*address),
                        corpus.registers.bits(*address & !3, usage.bits),
                        info.origin,
                        info.name
                    ));
                }
            }
            Ok(())
        },
    )?;
    budget.report("register-accesses", read_limits)?;
    let mut text: String = lines.into_iter().map(|line| format!("{line}\n")).collect();
    for function in symbolic {
        if let Some(dictionary) = dictionaries.get(&function) {
            text.push_str(dictionary);
        }
    }
    Ok(text)
}

fn conversions(format: &str) -> Vec<String> {
    let mut conversions = vec![];
    let mut current = String::new();
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        if c != '%' {
            continue;
        }
        if chars.peek() == Some(&'%') {
            current.extend(chars.next());
            continue;
        }
        while chars
            .peek()
            .is_some_and(|c| "-+ #0123456789.*lhzjtqL".contains(*c))
        {
            current.extend(chars.next());
        }
        if let Some(c) = chars.next() {
            current.push(c);
            if c.is_ascii_alphabetic() {
                conversions.push(std::mem::take(&mut current));
            }
        }
    }
    conversions
}
