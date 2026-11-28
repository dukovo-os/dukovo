// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::*;

use bits::*;

/// An Intel family 6 processor with AVX and RDRAND, two threads a core,
/// and nothing else said.
fn intel(model: u32, stepping: u32) -> Cpu {
    Cpu {
        vendor: Vendor::Intel,
        family: 6,
        model,
        stepping,
        microcode: 0,
        leaf1_ecx: LEAF1_AVX | LEAF1_RDRAND,
        leaf7_ebx: 0,
        leaf7_edx: 0,
        leaf7_2_edx: 0,
        ext8_ebx: 0,
        phys_bits: 39,
        memory_end: 16 << 30,
        ext21_eax: 0,
        ext21_ecx: 0,
        arch_capabilities: 0,
        mcu_opt_ctrl: None,
        smt: true,
    }
}

/// An AMD processor with AVX, IBPB, IBRS, STIBP and SSBD, two threads a
/// core.
fn amd(family: u32, model: u32, stepping: u32) -> Cpu {
    Cpu {
        vendor: Vendor::Amd,
        family,
        model,
        stepping,
        leaf7_ebx: LEAF7_SMAP,
        ext8_ebx: EXT8_IBPB | EXT8_IBRS | EXT8_STIBP | EXT8_SSBD,
        ..intel(model, stepping)
    }
}

/// Spectre-era Intel microcode: IBRS, IBPB, STIBP, SSBD, L1D_FLUSH,
/// MD_CLEAR.
const INTEL_SPEC: u32 =
    LEAF7_IBRS_IBPB | LEAF7_STIBP | LEAF7_SSBD | LEAF7_L1D_FLUSH | LEAF7_MD_CLEAR;

/// The OptiPlex 7040's Core i5-6500: Skylake, four cores, one thread each,
/// with TSX, and microcode with SRBDS's and GDS's controls.
fn i5_6500() -> Cpu {
    Cpu {
        leaf7_ebx: LEAF7_RTM,
        leaf7_edx: INTEL_SPEC | LEAF7_SRBDS_CTRL | LEAF7_ARCH_CAPABILITIES,
        arch_capabilities: caps::GDS_CTRL,
        mcu_opt_ctrl: Some(0),
        smt: false,
        ..intel(0x5e, 3)
    }
}

/// The R740's Xeon Gold 6152: Skylake-SP stepping 4, two threads a core.
fn gold_6152() -> Cpu {
    Cpu {
        leaf7_ebx: LEAF7_RTM,
        leaf7_edx: INTEL_SPEC | LEAF7_ARCH_CAPABILITIES,
        arch_capabilities: caps::GDS_CTRL,
        mcu_opt_ctrl: Some(0),
        ..intel(0x55, 4)
    }
}

/// The Precision 3660's Core i9-12900: Alder Lake with Atom cores.
fn i9_12900() -> Cpu {
    Cpu {
        leaf7_ebx: LEAF7_SMAP,
        leaf7_edx: INTEL_SPEC | LEAF7_ARCH_CAPABILITIES | LEAF7_HYBRID,
        leaf7_2_edx: LEAF7_2_RRSBA_CTRL | LEAF7_2_BHI_CTRL,
        arch_capabilities: caps::RDCL_NO
            | caps::IBRS_ALL
            | caps::MDS_NO
            | caps::PSCHANGE_MC_NO
            | caps::TAA_NO
            | caps::SBDR_SSDP_NO
            | caps::FBSDP_NO
            | caps::PSDP_NO
            | caps::RRSBA
            | caps::GDS_NO
            | caps::RFDS_CLEAR,
        ..intel(0x97, 2)
    }
}

/// The M75q's Ryzen 4000: Zen 2 (Renoir), on microcode older than its
/// Zenbleed fix.
fn renoir() -> Cpu {
    Cpu { microcode: 0x0860_0109, ..amd(0x17, 0x60, 1) }
}

/// A Ryzen 5000: Zen 3, with the microcode for SRSO and TSA.
fn zen3() -> Cpu {
    Cpu {
        ext8_ebx: amd(0x19, 0x21, 0).ext8_ebx | EXT8_STIBP_ALWAYS_ON | EXT8_BTC_NO,
        ext21_eax: EXT21_VERW_CLEAR | EXT21_IBPB_BRTYPE,
        ..amd(0x19, 0x21, 0)
    }
}

/// A Ryzen 7000: Zen 4, with automatic IBRS.
fn zen4() -> Cpu {
    Cpu {
        ext8_ebx: amd(0x19, 0x61, 2).ext8_ebx | EXT8_STIBP_ALWAYS_ON | EXT8_BTC_NO,
        ext21_eax: EXT21_VERW_CLEAR | EXT21_IBPB_BRTYPE | EXT21_AUTOIBRS,
        ..amd(0x19, 0x61, 2)
    }
}

fn choice(flags: &str) -> Choice {
    let mut choice = Choice::default();
    for flag in flags.split_whitespace() {
        let (name, value) = match flag.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (flag, None),
        };
        assert_eq!(choice.set(name, value), Ok(true), "{flag}");
    }
    choice
}

/// The plan of the kernel as it is built, with its thunks and page-table
/// isolation.
fn default_plan(cpu: &Cpu) -> Plan {
    Plan::make(cpu, &Choice::default(), KERNEL)
}

fn line(plan: &Plan, flaw: Flaw) -> String {
    let mut text = String::new();
    plan.write_line(flaw, &mut text).unwrap();
    text
}

fn affected(cpu: &Cpu) -> Vec<&'static str> {
    let found = find(cpu);
    Flaw::ALL.into_iter().filter(|&flaw| found.has(flaw)).map(Flaw::name).collect()
}

#[test]
fn every_flaw_has_a_unique_name_listed_in_order() {
    let names: Vec<_> = Flaw::ALL.iter().map(|flaw| flaw.name()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(names, sorted);
    for flaw in Flaw::ALL {
        assert_eq!(Flaw::named(flaw.name()), Some(flaw));
    }
    assert_eq!(Flaw::named("spectre_v3"), None);
}

#[test]
fn the_i5_6500_has_skylakes_flaws_and_takes_ibrs_on_entry() {
    let cpu = i5_6500();
    assert_eq!(
        affected(&cpu),
        [
            "gather_data_sampling",
            "itlb_multihit",
            "l1tf",
            "mds",
            "meltdown",
            "mmio_stale_data",
            "retbleed",
            "spec_store_bypass",
            "spectre_v1",
            "spectre_v2",
            "srbds",
            "tsx_async_abort",
            "vmscape",
        ]
    );
    let plan = default_plan(&cpu);
    let controls = plan.controls();
    assert_eq!(plan.spectre_v2(), SpectreV2::Ibrs);
    assert_eq!(controls.spec_ctrl_kernel, SPEC_CTRL_IBRS);
    assert_eq!(controls.spec_ctrl_user, 0);
    assert!(controls.verw_on_exit && !controls.verw_on_idle);
    assert!(controls.swapgs_fence && controls.rsb_fill_on_switch);
    assert!(!controls.bhb_clear && !controls.ibrs_firmware && !controls.tsx_disable);
    assert_eq!(controls.mcu_opt_clear, mcu_opt::RNGDS_MITG_DIS | mcu_opt::GDS_MITG_DIS);
    assert_eq!(controls.mcu_opt_set, 0);
    assert_eq!(
        line(&plan, Flaw::SpectreV2),
        "Mitigation: IBRS; IBPB: disabled; STIBP: disabled; RSB filling; PBRSB-eIBRS: Not \
         affected; BHI: Not affected"
    );
    assert_eq!(line(&plan, Flaw::Retbleed), "Mitigation: IBRS");
    assert_eq!(line(&plan, Flaw::Mds), "Mitigation: Clear CPU buffers; SMT disabled");
    assert_eq!(line(&plan, Flaw::TsxAsyncAbort), "Mitigation: Clear CPU buffers; SMT disabled");
    assert_eq!(line(&plan, Flaw::MmioStaleData), "Mitigation: Clear CPU buffers; SMT disabled");
    assert_eq!(line(&plan, Flaw::Srbds), "Mitigation: Microcode");
    assert_eq!(line(&plan, Flaw::GatherDataSampling), "Mitigation: Microcode");
    assert_eq!(line(&plan, Flaw::L1tf), "Mitigation: PTE Inversion");
    assert!(controls.pti);
    assert_eq!(line(&plan, Flaw::Meltdown), "Mitigation: PTI");
    assert_eq!(line(&plan, Flaw::SpecStoreBypass), "Vulnerable");
    assert_eq!(line(&plan, Flaw::ItlbMultihit), "Mitigation: VMX disabled");
    assert_eq!(line(&plan, Flaw::Vmscape), "Mitigation: no guests run");
    assert_eq!(line(&plan, Flaw::RegFileDataSampling), "Not affected");
}

#[test]
fn the_gold_6152_keeps_its_threads_but_says_they_are_open() {
    let cpu = gold_6152();
    let found = find(&cpu);
    assert!(found.has(Flaw::Retbleed) && found.has(Flaw::MmioStaleData));
    assert!(!found.has(Flaw::Srbds), "Skylake-SP has no SRBDS");
    assert!(!found.has(Flaw::IndirectTargetSelection), "stepping 4 is before ITS");
    let plan = default_plan(&cpu);
    assert!(!plan.controls().nosmt);
    assert_eq!(line(&plan, Flaw::Mds), "Mitigation: Clear CPU buffers; SMT vulnerable");
    assert_eq!(
        line(&plan, Flaw::SpectreV2),
        "Mitigation: IBRS; IBPB: disabled; STIBP: disabled; RSB filling; PBRSB-eIBRS: Not \
         affected; BHI: Not affected"
    );
    let later = Cpu { stepping: 7, ..cpu };
    assert!(find(&later).has(Flaw::IndirectTargetSelection), "Cascade Lake has ITS");
}

#[test]
fn auto_nosmt_turns_threads_off_only_where_a_flaw_crosses_them() {
    let nosmt = choice("mitigations=auto,nosmt");
    let gold = Plan::make(&gold_6152(), &nosmt, Built::default());
    assert!(gold.controls().nosmt);
    assert_eq!(line(&gold, Flaw::Mds), "Mitigation: Clear CPU buffers; SMT disabled");
    let alder = Plan::make(&i9_12900(), &nosmt, Built::default());
    assert!(!alder.controls().nosmt);
    let always = Plan::make(&i9_12900(), &choice("nosmt"), Built::default());
    assert!(always.controls().nosmt);
    let single = Plan::make(&i5_6500(), &choice("nosmt"), Built::default());
    assert!(!single.controls().nosmt, "nothing to turn off");
}

#[test]
fn the_i9_12900_uses_enhanced_ibrs_bhi_dis_s_and_clears_the_register_file() {
    let cpu = i9_12900();
    assert_eq!(
        affected(&cpu),
        ["reg_file_data_sampling", "spec_store_bypass", "spectre_v1", "spectre_v2", "vmscape"]
    );
    let plan = default_plan(&cpu);
    let controls = plan.controls();
    let base = SPEC_CTRL_IBRS | SPEC_CTRL_RRSBA_DIS_S | SPEC_CTRL_BHI_DIS_S;
    assert_eq!(controls.spec_ctrl_kernel, base);
    assert_eq!(controls.spec_ctrl_user, base);
    assert!(controls.verw_on_exit && !controls.verw_on_idle && !controls.bhb_clear);
    assert!(!controls.auto_ibrs && !controls.ibrs_firmware);
    assert!(!controls.swapgs_fence, "SMAP works, and programs cannot point GS at the kernel");
    let fsgsbase = Built { fsgsbase: true, ..Built::default() };
    assert!(Plan::make(&cpu, &Choice::default(), fsgsbase).controls().swapgs_fence);
    assert_eq!(
        line(&plan, Flaw::SpectreV2),
        "Mitigation: Enhanced / Automatic IBRS; IBPB: disabled; RSB filling; PBRSB-eIBRS: no \
         guests run; BHI: BHI_DIS_S"
    );
    assert_eq!(line(&plan, Flaw::RegFileDataSampling), "Mitigation: Clear Register File");
    assert_eq!(line(&plan, Flaw::Retbleed), "Not affected");
    assert_eq!(line(&plan, Flaw::Meltdown), "Not affected");
}

#[test]
fn alder_lake_without_atom_cores_has_no_rfds_but_its_mobile_parts_do() {
    let mut cpu = i9_12900();
    cpu.leaf7_edx &= !LEAF7_HYBRID;
    cpu.arch_capabilities &= !caps::RFDS_CLEAR;
    assert!(!find(&cpu).has(Flaw::RegFileDataSampling));
    cpu.model = 0x9a;
    assert!(find(&cpu).has(Flaw::RegFileDataSampling));
    assert_eq!(line(&default_plan(&cpu), Flaw::RegFileDataSampling), "Vulnerable: No microcode");
    cpu.arch_capabilities |= caps::RFDS_NO;
    assert!(!find(&cpu).has(Flaw::RegFileDataSampling));
}

#[test]
fn enhanced_ibrs_without_bhi_dis_s_clears_the_history_on_system_calls() {
    let mut cpu = i9_12900();
    cpu.leaf7_2_edx = 0;
    let plan = default_plan(&cpu);
    assert!(plan.controls().bhb_clear);
    assert_eq!(plan.controls().spec_ctrl_kernel, SPEC_CTRL_IBRS);
    assert!(line(&plan, Flaw::SpectreV2).ends_with("; BHI: SW loop"));
    // One that says BHI_NO needs neither, as on Linux.
    cpu.arch_capabilities |= caps::BHI_NO;
    let plan = default_plan(&cpu);
    assert!(!find(&cpu).bhi);
    assert!(!plan.controls().bhb_clear);
    assert!(line(&plan, Flaw::SpectreV2).ends_with("; BHI: Not affected"));
}

#[test]
fn renoir_takes_retpolines_and_the_safe_return_and_the_zenbleed_chicken_bit() {
    let cpu = renoir();
    assert_eq!(
        affected(&cpu),
        [
            "retbleed",
            "spec_rstack_overflow",
            "spec_store_bypass",
            "spectre_v1",
            "spectre_v2",
            "vmscape",
        ]
    );
    let plan = default_plan(&cpu);
    let controls = plan.controls();
    assert_eq!(plan.spectre_v2(), SpectreV2::Retpoline);
    assert!(controls.zenbleed_chicken && !controls.div0_clear);
    assert!(controls.ibpb_firmware && !controls.ibrs_firmware);
    assert!(controls.rsb_fill_on_switch && !controls.verw_on_exit && !controls.swapgs_fence);
    // The untrained return needs STIBP while both threads run.
    assert_eq!(controls.spec_ctrl_user, SPEC_CTRL_STIBP);
    assert_eq!(
        line(&plan, Flaw::SpectreV2),
        "Mitigation: Retpolines; IBPB: disabled; STIBP: always-on; RSB filling; PBRSB-eIBRS: Not \
         affected; BHI: Not affected"
    );
    assert_eq!(
        line(&plan, Flaw::Retbleed),
        "Mitigation: untrained return thunk; SMT enabled with STIBP protection"
    );
    assert_eq!(line(&plan, Flaw::SpecRstackOverflow), "Mitigation: Safe RET");
    assert_eq!(line(&plan, Flaw::Mds), "Not affected");
    // A kernel without the thunks leaves all three open.
    let unbuilt = Plan::make(&cpu, &Choice::default(), Built::DEFAULT);
    assert_eq!(unbuilt.spectre_v2(), SpectreV2::Off);
    assert_eq!(unbuilt.controls().spec_ctrl_kernel, 0);
    assert_eq!(
        line(&unbuilt, Flaw::SpectreV2),
        "Vulnerable; IBPB: disabled; STIBP: disabled; RSB filling; PBRSB-eIBRS: Not affected; \
         BHI: Not affected"
    );
    assert_eq!(line(&unbuilt, Flaw::Retbleed), "Vulnerable");
    assert_eq!(line(&unbuilt, Flaw::SpecRstackOverflow), "Vulnerable");
    let fixed = Cpu { microcode: 0x0860_010c, ..cpu };
    assert!(!find(&fixed).zenbleed);
    let guest = Cpu { leaf1_ecx: cpu.leaf1_ecx | LEAF1_HYPERVISOR, ..cpu };
    assert!(!find(&guest).zenbleed);
}

#[test]
fn retbleed_ibpb_on_zen_2_closes_both_return_flaws_with_stibp() {
    let plan = Plan::make(&renoir(), &choice("retbleed=ibpb"), Built::default());
    let controls = plan.controls();
    assert!(controls.ibpb_on_entry && !controls.ibpb_firmware);
    assert_eq!(controls.spec_ctrl_user, SPEC_CTRL_STIBP);
    assert_eq!(line(&plan, Flaw::Retbleed), "Mitigation: IBPB; SMT enabled with STIBP protection");
    assert_eq!(line(&plan, Flaw::SpecRstackOverflow), "Mitigation: IBPB");
    assert!(line(&plan, Flaw::SpectreV2).contains("; STIBP: always-on"));
}

#[test]
fn srso_ibpb_needs_microcode_that_flushes_returns() {
    let mut cpu = zen3();
    cpu.ext21_eax &= !EXT21_IBPB_BRTYPE;
    let plan = Plan::make(&cpu, &choice("spec_rstack_overflow=ibpb"), Built::default());
    assert!(!plan.controls().ibpb_on_entry);
    assert_eq!(line(&plan, Flaw::SpecRstackOverflow), "Vulnerable: No microcode");
    let plan = Plan::make(&zen3(), &choice("spec_rstack_overflow=ibpb"), Built::default());
    assert!(plan.controls().ibpb_on_entry);
    assert_eq!(line(&plan, Flaw::SpecRstackOverflow), "Mitigation: IBPB");
}

#[test]
fn zen_1_and_2_without_threads_are_out_of_srsos_reach() {
    let cpu = Cpu { smt: false, ..renoir() };
    assert_eq!(line(&default_plan(&cpu), Flaw::SpecRstackOverflow), "Mitigation: SMT disabled");
    // Zen 3 is reached with one thread a core too.
    let zen3 = Cpu { smt: false, ..zen3() };
    assert_eq!(line(&default_plan(&zen3), Flaw::SpecRstackOverflow), "Mitigation: Safe RET");
}

#[test]
fn zen_1_clears_the_divider_on_exit() {
    let plan = default_plan(&amd(0x17, 0x08, 2));
    assert!(plan.controls().div0_clear && !plan.controls().zenbleed_chicken);
}

#[test]
fn zen_3_keeps_stibp_on_and_clears_buffers_for_tsa() {
    let cpu = zen3();
    let found = find(&cpu);
    assert!(found.has(Flaw::Tsa) && !found.has(Flaw::Retbleed));
    let plan = default_plan(&cpu);
    let controls = plan.controls();
    assert_eq!(controls.spec_ctrl_user, SPEC_CTRL_STIBP);
    assert!(controls.verw_on_exit && controls.verw_on_idle && controls.ibrs_firmware);
    assert_eq!(line(&plan, Flaw::Tsa), "Mitigation: Clear CPU buffers: user/kernel boundary");
    assert_eq!(
        line(&plan, Flaw::SpectreV2),
        "Mitigation: Retpolines; IBPB: disabled; IBRS_FW; STIBP: always-on; RSB filling; \
         PBRSB-eIBRS: Not affected; BHI: Not affected"
    );
    let old = Cpu { ext21_eax: 0, ..cpu };
    assert_eq!(line(&default_plan(&old), Flaw::Tsa), "Vulnerable: No microcode");
    let free = Cpu { ext21_ecx: EXT21_TSA_SQ_NO | EXT21_TSA_L1_NO, ..cpu };
    assert!(!find(&free).has(Flaw::Tsa));
}

#[test]
fn zen_4_uses_automatic_ibrs() {
    let plan = default_plan(&zen4());
    let controls = plan.controls();
    assert_eq!(plan.spectre_v2(), SpectreV2::Eibrs);
    assert!(controls.auto_ibrs && !controls.ibrs_firmware);
    assert_eq!(controls.spec_ctrl_kernel, SPEC_CTRL_STIBP);
    assert!(line(&plan, Flaw::SpectreV2).starts_with(
        "Mitigation: Enhanced / Automatic IBRS; IBPB: disabled; STIBP: always-on; RSB filling"
    ));
}

#[test]
fn zen_5_reports_srso_closed_while_no_guests_run_and_has_no_tsa() {
    let cpu = Cpu {
        ext21_eax: zen4().ext21_eax | EXT21_SRSO_USER_KERNEL_NO,
        ..Cpu { family: 0x1a, model: 0x44, ..zen4() }
    };
    let found = find(&cpu);
    assert!(!found.has(Flaw::Tsa));
    assert_eq!(line(&default_plan(&cpu), Flaw::SpecRstackOverflow), "Mitigation: no guests run");
    let guest = Cpu { leaf1_ecx: cpu.leaf1_ecx | LEAF1_HYPERVISOR, ..cpu };
    assert!(find(&guest).has(Flaw::Tsa), "a Zen guest may move to a host with TSA");
}

#[test]
fn retpolines_and_return_thunks_close_what_s1_leaves_open_on_zen_2() {
    let built = Built { retpoline: true, return_thunks: true, ..Built::default() };
    let plan = Plan::make(&renoir(), &Choice::default(), built);
    assert_eq!(plan.spectre_v2(), SpectreV2::Retpoline);
    assert_eq!(
        line(&plan, Flaw::Retbleed),
        "Mitigation: untrained return thunk; SMT enabled with STIBP protection"
    );
    assert_eq!(line(&plan, Flaw::SpecRstackOverflow), "Mitigation: Safe RET");
    // Skylake stays on IBRS, which retpolines cannot replace there.
    let skylake = Plan::make(&i5_6500(), &Choice::default(), built);
    assert_eq!(skylake.spectre_v2(), SpectreV2::Ibrs);
    // Haswell has no Retbleed, so it moves to retpolines.
    let haswell = Cpu { leaf7_edx: INTEL_SPEC, ..intel(0x3c, 3) };
    assert_eq!(
        Plan::make(&haswell, &Choice::default(), Built::default()).spectre_v2(),
        SpectreV2::Ibrs
    );
    assert_eq!(Plan::make(&haswell, &Choice::default(), built).spectre_v2(), SpectreV2::Retpoline);
}

/// What the kernel is built with since retpolines and return thunks.
const THUNKS: Built = Built { retpoline: true, return_thunks: true, ..Built::DEFAULT };
/// The kernel as it is built.
const KERNEL: Built = Built { pti: true, ..THUNKS };

fn paths(plan: &Plan) -> (IndirectBranches, ReturnPath) {
    (plan.controls().indirect_branches, plan.controls().returns)
}

#[test]
fn zen_2_runs_retpolines_and_the_safe_return_which_closes_both_flaws_of_returns() {
    let plan = Plan::make(&renoir(), &Choice::default(), THUNKS);
    assert_eq!(paths(&plan), (IndirectBranches::Retpoline, ReturnPath::SafeRet));
    assert!(plan.controls().returns.untrains());
    // Without SRSO's closing, Retbleed's untrained return is enough.
    let no_srso = Plan::make(&renoir(), &choice("spec_rstack_overflow=off"), THUNKS);
    assert_eq!(paths(&no_srso), (IndirectBranches::Retpoline, ReturnPath::Untrained));
    assert_eq!(line(&no_srso, Flaw::SpecRstackOverflow), "Vulnerable");
    // With one thread a core, SRSO cannot reach the kernel.
    let one_thread = Plan::make(&Cpu { smt: false, ..renoir() }, &Choice::default(), THUNKS);
    assert_eq!(paths(&one_thread).1, ReturnPath::Untrained);
    assert_eq!(line(&one_thread, Flaw::SpecRstackOverflow), "Mitigation: SMT disabled");
    // IBPB on every entry closes both, and returns run plain.
    let ibpb = Plan::make(&renoir(), &choice("retbleed=ibpb"), THUNKS);
    assert_eq!(paths(&ibpb), (IndirectBranches::Retpoline, ReturnPath::Plain));
    assert!(!ibpb.controls().returns.untrains());
}

#[test]
fn zen_3_and_4_take_the_safe_return_at_aliased_addresses() {
    let zen3 = Plan::make(&zen3(), &Choice::default(), THUNKS);
    assert_eq!(paths(&zen3), (IndirectBranches::Retpoline, ReturnPath::SafeRetAliased));
    assert_eq!(line(&zen3, Flaw::SpecRstackOverflow), "Mitigation: Safe RET");
    assert!(line(&zen3, Flaw::SpectreV2).starts_with("Mitigation: Retpolines; "));
    // Automatic IBRS guards Zen 4's indirect branches, not its returns.
    let zen4 = Plan::make(&zen4(), &Choice::default(), THUNKS);
    assert_eq!(paths(&zen4), (IndirectBranches::Plain, ReturnPath::SafeRetAliased));
}

#[test]
fn intel_s_own_protections_leave_branches_and_returns_plain() {
    // Skylake: IBRS on every entry.
    let skylake = Plan::make(&i5_6500(), &Choice::default(), THUNKS);
    assert_eq!(skylake.spectre_v2(), SpectreV2::Ibrs);
    assert_eq!(paths(&skylake), (IndirectBranches::Plain, ReturnPath::Plain));
    // Alder Lake: enhanced IBRS, unless retpolines are asked for.
    let alder = Plan::make(&i9_12900(), &Choice::default(), THUNKS);
    assert_eq!(paths(&alder), (IndirectBranches::Plain, ReturnPath::Plain));
    for flag in ["spectre_v2=retpoline", "spectre_v2=retpoline,generic"] {
        let asked = Plan::make(&i9_12900(), &choice(flag), THUNKS);
        assert_eq!(asked.spectre_v2(), SpectreV2::Retpoline, "{flag}");
        assert_eq!(paths(&asked), (IndirectBranches::Retpoline, ReturnPath::Plain), "{flag}");
    }
    // A kernel without retpolines cannot be asked for them.
    let unbuilt = Plan::make(&i9_12900(), &choice("spectre_v2=retpoline"), Built::DEFAULT);
    assert_eq!(unbuilt.spectre_v2(), SpectreV2::Eibrs);
}

#[test]
fn its_aligns_branches_and_returns_unless_it_is_turned_off() {
    let icelake = Cpu {
        leaf7_edx: INTEL_SPEC | LEAF7_ARCH_CAPABILITIES,
        arch_capabilities: caps::RDCL_NO
            | caps::IBRS_ALL
            | caps::MDS_NO
            | caps::TAA_NO
            | caps::PSCHANGE_MC_NO,
        ..intel(0x6a, 6)
    };
    assert!(find(&icelake).has(Flaw::IndirectTargetSelection));
    let plan = Plan::make(&icelake, &Choice::default(), THUNKS);
    assert_eq!(plan.spectre_v2(), SpectreV2::Eibrs);
    assert_eq!(paths(&plan), (IndirectBranches::Aligned, ReturnPath::Aligned));
    assert_eq!(
        line(&plan, Flaw::IndirectTargetSelection),
        "Mitigation: Aligned branch/return thunks"
    );
    // With retpolines there are no indirect branches left to align.
    let retpoline = Plan::make(&icelake, &choice("spectre_v2=retpoline"), THUNKS);
    assert_eq!(paths(&retpoline), (IndirectBranches::Retpoline, ReturnPath::Aligned));
    for flags in ["indirect_target_selection=off", "spectre_v2=off", "mitigations=off"] {
        let off = Plan::make(&icelake, &choice(flags), THUNKS);
        assert_eq!(paths(&off).1, ReturnPath::Plain, "{flags}");
        assert_eq!(line(&off, Flaw::IndirectTargetSelection), "Vulnerable", "{flags}");
    }
    let asked =
        Plan::make(&icelake, &choice("mitigations=off indirect_target_selection=on"), THUNKS);
    assert_eq!(paths(&asked), (IndirectBranches::Plain, ReturnPath::Plain), "spectre_v2 is off");
    // A kernel without the thunks cannot align them.
    let unbuilt = Plan::make(&icelake, &Choice::default(), Built::DEFAULT);
    assert_eq!(paths(&unbuilt), (IndirectBranches::Plain, ReturnPath::Plain));
    assert_eq!(line(&unbuilt, Flaw::IndirectTargetSelection), "Vulnerable");
}

#[test]
fn mitigations_off_leaves_branches_and_returns_plain_everywhere() {
    for cpu in [renoir(), zen3(), zen4(), i5_6500(), gold_6152(), i9_12900()] {
        let plan = Plan::make(&cpu, &choice("mitigations=off"), THUNKS);
        assert_eq!(paths(&plan), (IndirectBranches::Plain, ReturnPath::Plain), "{cpu:?}");
    }
    // Linux's names for the return thunks ask for them even then.
    let unret = Plan::make(&renoir(), &choice("mitigations=off retbleed=unret"), THUNKS);
    assert_eq!(paths(&unret), (IndirectBranches::Plain, ReturnPath::Untrained));
    let safe_ret = choice("mitigations=off spec_rstack_overflow=safe-ret");
    assert_eq!(paths(&Plan::make(&zen3(), &safe_ret, THUNKS)).1, ReturnPath::SafeRetAliased);
}

#[test]
fn spectre_v2_on_and_ibrs_force_ibrs_on_zen_2() {
    for flags in ["spectre_v2=on", "spectre_v2=ibrs"] {
        let plan = Plan::make(&renoir(), &choice(flags), Built::default());
        assert_eq!(plan.spectre_v2(), SpectreV2::Ibrs, "{flags}");
        assert_eq!(plan.controls().spec_ctrl_kernel & SPEC_CTRL_IBRS, SPEC_CTRL_IBRS);
        assert_eq!(plan.controls().spec_ctrl_user & SPEC_CTRL_IBRS, 0);
    }
    let on = Plan::make(&renoir(), &choice("spectre_v2=on"), Built::default());
    assert!(on.controls().ibpb_on_switch, "spectre_v2=on means spectre_v2_user=on");
    assert!(line(&on, Flaw::SpectreV2).contains("; IBPB: always-on; STIBP: forced"));
}

#[test]
fn spectre_v2_user_on_adds_ibpb_on_switch_and_stibp() {
    let plan = Plan::make(&gold_6152(), &choice("spectre_v2_user=on"), Built::default());
    let controls = plan.controls();
    assert!(controls.ibpb_on_switch);
    assert_eq!(controls.spec_ctrl_user, SPEC_CTRL_STIBP);
    assert_eq!(controls.spec_ctrl_kernel, SPEC_CTRL_IBRS | SPEC_CTRL_STIBP);
}

#[test]
fn spec_store_bypass_disable_on_sets_ssbd_everywhere() {
    let plan = Plan::make(&i9_12900(), &choice("spec_store_bypass_disable=on"), Built::default());
    assert_eq!(plan.controls().spec_ctrl_user & SPEC_CTRL_SSBD, SPEC_CTRL_SSBD);
    assert_eq!(line(&plan, Flaw::SpecStoreBypass), "Mitigation: Speculative Store Bypass disabled");
}

#[test]
fn meltdown_is_closed_by_page_table_isolation_unless_it_is_turned_off() {
    let isolated = |cpu: &Cpu, flags: &str| {
        let plan = Plan::make(cpu, &choice(flags), KERNEL);
        (plan.controls().pti, line(&plan, Flaw::Meltdown))
    };
    let skylake = i5_6500();
    assert_eq!(isolated(&skylake, ""), (true, "Mitigation: PTI".into()));
    assert_eq!(isolated(&skylake, "pti=auto"), (true, "Mitigation: PTI".into()));
    for off in ["pti=off", "nopti", "mitigations=off", "pti=on nopti"] {
        assert_eq!(isolated(&skylake, off), (false, "Vulnerable".into()), "{off}");
    }
    assert_eq!(isolated(&skylake, "mitigations=off pti=on"), (true, "Mitigation: PTI".into()));
    // Processors without Meltdown run without it, unless it is asked for.
    assert_eq!(isolated(&i9_12900(), ""), (false, "Not affected".into()));
    assert_eq!(isolated(&zen3(), ""), (false, "Not affected".into()));
    assert_eq!(isolated(&zen3(), "pti=on"), (true, "Not affected".into()));
    // A kernel that cannot isolate itself says so.
    let plan = Plan::make(&skylake, &choice("pti=on"), THUNKS);
    assert!(!plan.controls().pti);
    assert_eq!(line(&plan, Flaw::Meltdown), "Vulnerable");
    assert_eq!(Choice::default().set("pti", Some("yes")), Err(BadValue));
    assert_eq!(Choice::default().set("nopti", Some("1")), Err(BadValue));
}

#[test]
fn mitigations_off_leaves_only_the_free_and_errata_fixes() {
    let off = choice("mitigations=off");
    let plan = Plan::make(&i5_6500(), &off, Built::default());
    let controls = plan.controls();
    assert_eq!(controls.spec_ctrl_kernel, 0);
    assert!(!controls.verw_on_exit && !controls.swapgs_fence && !controls.rsb_fill_on_switch);
    assert_eq!(controls.mcu_opt_set, mcu_opt::RNGDS_MITG_DIS | mcu_opt::GDS_MITG_DIS);
    assert_eq!(line(&plan, Flaw::SpectreV2).split(';').next(), Some("Vulnerable"));
    assert_eq!(line(&plan, Flaw::Mds), "Vulnerable; SMT disabled");
    assert_eq!(line(&plan, Flaw::Srbds), "Vulnerable");
    assert_eq!(line(&plan, Flaw::L1tf), "Mitigation: PTE Inversion");
    assert!(line(&plan, Flaw::SpectreV1).starts_with("Vulnerable: __user pointer"));
    let renoir = Plan::make(&renoir(), &off, Built::default());
    assert!(renoir.controls().zenbleed_chicken, "an erratum fix, not a choice");
    // A protection asked for by name still runs.
    let ibrs = Plan::make(&i5_6500(), &choice("mitigations=off spectre_v2=ibrs"), Built::default());
    assert_eq!(ibrs.spectre_v2(), SpectreV2::Ibrs);
}

#[test]
fn one_verw_flag_off_leaves_buffers_cleared_for_the_others() {
    let plan = Plan::make(&i5_6500(), &choice("mds=off"), Built::default());
    assert!(plan.controls().verw_on_exit);
    assert_eq!(line(&plan, Flaw::Mds), "Mitigation: Clear CPU buffers; SMT disabled");
    let flags = "mds=off tsx_async_abort=off mmio_stale_data=off";
    let plan = Plan::make(&i5_6500(), &choice(flags), Built::default());
    assert!(!plan.controls().verw_on_exit);
    assert!(plan.is_vulnerable(Flaw::Mds));
}

#[test]
fn tsx_goes_off_wherever_tsx_ctrl_allows_and_closes_taa() {
    // Cascade Lake: MDS_NO, but TAA through TSX until it is turned off.
    let cascade = Cpu {
        leaf7_ebx: LEAF7_RTM,
        leaf7_edx: INTEL_SPEC | LEAF7_ARCH_CAPABILITIES,
        arch_capabilities: caps::RDCL_NO
            | caps::IBRS_ALL
            | caps::MDS_NO
            | caps::TSX_CTRL
            | caps::GDS_CTRL,
        mcu_opt_ctrl: Some(0),
        ..intel(0x55, 7)
    };
    let found = find(&cascade);
    assert!(found.has(Flaw::TsxAsyncAbort) && !found.has(Flaw::Mds));
    let plan = default_plan(&cascade);
    assert!(plan.controls().tsx_disable);
    assert_eq!(line(&plan, Flaw::TsxAsyncAbort), "Mitigation: TSX disabled");
    // MMIO stale data still wants the buffers cleared.
    assert!(plan.controls().verw_on_exit && plan.controls().verw_on_idle);
    assert_eq!(line(&plan, Flaw::Retbleed), "Mitigation: Enhanced IBRS");
    let kept = Plan::make(&cascade, &choice("tsx=on"), Built::default());
    assert!(!kept.controls().tsx_disable);
    assert_eq!(line(&kept, Flaw::TsxAsyncAbort), "Mitigation: Clear CPU buffers; SMT vulnerable");
}

#[test]
fn srbds_is_closed_by_tsx_off_on_mds_free_parts_without_mmio() {
    // Coffee Lake Refresh with TSX and IA32_TSX_CTRL, free of MMIO.
    let cpu = Cpu {
        leaf7_ebx: LEAF7_RTM,
        leaf7_edx: INTEL_SPEC | LEAF7_SRBDS_CTRL | LEAF7_ARCH_CAPABILITIES,
        arch_capabilities: caps::RDCL_NO
            | caps::IBRS_ALL
            | caps::MDS_NO
            | caps::TSX_CTRL
            | caps::SBDR_SSDP_NO
            | caps::FBSDP_NO
            | caps::PSDP_NO,
        mcu_opt_ctrl: Some(0),
        ..intel(0x9e, 0x0d)
    };
    let plan = default_plan(&cpu);
    assert_eq!(line(&plan, Flaw::Srbds), "Mitigation: TSX disabled");
    assert_eq!(plan.controls().mcu_opt_set, mcu_opt::RNGDS_MITG_DIS);
}

#[test]
fn a_locked_gds_control_is_reported_as_it_stands() {
    let locked_on = Cpu { mcu_opt_ctrl: Some(mcu_opt::GDS_MITG_LOCKED), ..gold_6152() };
    let plan = Plan::make(&locked_on, &choice("gather_data_sampling=off"), Built::default());
    assert_eq!(line(&plan, Flaw::GatherDataSampling), "Mitigation: Microcode (locked)");
    assert_eq!(plan.controls().mcu_opt_set & mcu_opt::GDS_MITG_DIS, 0);
    let locked_off =
        Cpu { mcu_opt_ctrl: Some(mcu_opt::GDS_MITG_LOCKED | mcu_opt::GDS_MITG_DIS), ..gold_6152() };
    let plan = default_plan(&locked_off);
    assert_eq!(line(&plan, Flaw::GatherDataSampling), "Vulnerable");
    assert_eq!(plan.controls().mcu_opt_clear & mcu_opt::GDS_MITG_DIS, 0);
    let no_avx = Cpu { leaf1_ecx: LEAF1_RDRAND, ..gold_6152() };
    assert!(!find(&no_avx).has(Flaw::GatherDataSampling));
}

#[test]
fn a_guest_assumes_the_worst_of_its_host() {
    let guest = Cpu {
        leaf1_ecx: LEAF1_AVX | LEAF1_RDRAND | LEAF1_HYPERVISOR,
        leaf7_edx: INTEL_SPEC | LEAF7_ARCH_CAPABILITIES,
        arch_capabilities: caps::RDCL_NO | caps::MDS_NO | caps::TAA_NO | caps::PSCHANGE_MC_NO,
        ..intel(0x6a, 6)
    };
    let found = find(&guest);
    assert!(found.bhi, "a hypervisor may hide enhanced IBRS");
    let told = Cpu { arch_capabilities: guest.arch_capabilities | caps::BHI_NO, ..guest };
    assert!(!find(&told).bhi, "a hypervisor that says BHI_NO is believed, as on Linux");
    assert!(found.has(Flaw::IndirectTargetSelection));
    // Without enhanced IBRS it takes retpolines, which leave no indirect
    // branch for the history to steer; a kernel without them clears it.
    let plan = default_plan(&guest);
    assert_eq!(plan.spectre_v2(), SpectreV2::Retpoline);
    assert!(!plan.controls().bhb_clear);
    assert!(Plan::make(&guest, &Choice::default(), Built::DEFAULT).controls().bhb_clear);
    assert_eq!(line(&plan, Flaw::GatherDataSampling), "Unknown: Dependent on hypervisor status");
    let mds_guest = Cpu { leaf1_ecx: LEAF1_AVX | LEAF1_HYPERVISOR, ..gold_6152() };
    assert_eq!(
        line(&default_plan(&mds_guest), Flaw::Mds),
        "Mitigation: Clear CPU buffers; SMT Host state unknown"
    );
    let its_no = Cpu { arch_capabilities: guest.arch_capabilities | caps::ITS_NO, ..guest };
    assert!(!find(&its_no).has(Flaw::IndirectTargetSelection));
    let amd_guest = Cpu { leaf1_ecx: LEAF1_HYPERVISOR, ..renoir() };
    assert!(!find(&amd_guest).has(Flaw::IndirectTargetSelection));
}

#[test]
fn store_buffer_only_mds_is_closed_across_threads_by_verw_before_idle() {
    let knights = Cpu { leaf7_edx: INTEL_SPEC, ..intel(0x57, 1) };
    let found = find(&knights);
    assert!(found.msbds_only && !found.has(Flaw::L1tf) && !found.swapgs);
    let plan = default_plan(&knights);
    assert!(plan.controls().verw_on_idle);
    assert_eq!(line(&plan, Flaw::Mds), "Mitigation: Clear CPU buffers; SMT mitigated");
}

#[test]
fn in_order_atoms_have_no_flaws() {
    let cpu = intel(0x1c, 2);
    assert!(affected(&cpu).is_empty());
    let plan = default_plan(&cpu);
    assert_eq!(*plan.controls(), Controls::default());
    for flaw in Flaw::ALL {
        assert_eq!(line(&plan, flaw), "Not affected");
    }
}

#[test]
fn a_new_model_is_judged_by_what_it_says_of_itself() {
    let cpu = Cpu {
        leaf7_edx: INTEL_SPEC | LEAF7_ARCH_CAPABILITIES,
        leaf7_2_edx: LEAF7_2_BHI_CTRL,
        arch_capabilities: caps::RDCL_NO
            | caps::IBRS_ALL
            | caps::SSB_NO
            | caps::MDS_NO
            | caps::PSCHANGE_MC_NO
            | caps::TAA_NO
            | caps::SBDR_SSDP_NO
            | caps::FBSDP_NO
            | caps::PSDP_NO
            | caps::PBRSB_NO
            | caps::GDS_NO
            | caps::RFDS_NO
            | caps::ITS_NO,
        ..intel(0xee, 0)
    };
    assert_eq!(affected(&cpu), ["spectre_v1", "spectre_v2"]);
    let plan = default_plan(&cpu);
    assert!(line(&plan, Flaw::SpectreV2).contains("; PBRSB-eIBRS: Not affected; BHI: BHI_DIS_S"));
}

#[test]
fn every_line_starts_as_linux_tools_expect() {
    let cpus = [i5_6500(), gold_6152(), i9_12900(), renoir(), zen3(), zen4(), amd(0x17, 1, 1)];
    let choices = ["", "mitigations=off", "mitigations=auto,nosmt", "spectre_v2=on retbleed=ibpb"];
    for cpu in &cpus {
        for flags in choices {
            let plan = Plan::make(cpu, &choice(flags), Built::default());
            for flaw in Flaw::ALL {
                let text = line(&plan, flaw);
                assert!(
                    ["Not affected", "Vulnerable", "Mitigation: ", "Unknown: "]
                        .iter()
                        .any(|start| text.starts_with(start)),
                    "{} with {flags:?}: {text}",
                    flaw.name()
                );
                assert!(!text.contains('\n'));
                assert_eq!(plan.is_vulnerable(flaw), text.starts_with("Vulnerable"));
            }
        }
    }
}

#[test]
fn flags_are_checked_against_their_values() {
    let mut choice = Choice::default();
    assert_eq!(choice.set("loglevel", Some("debug")), Ok(false));
    assert_eq!(choice.set("spectre_v2", Some("retpoline,lfence")), Err(BadValue));
    assert_eq!(choice.set("indirect_target_selection", Some("force")), Err(BadValue));
    assert_eq!(choice.set("nosmt", Some("force")), Err(BadValue));
    assert_eq!(choice.set("retbleed", None), Err(BadValue));
    assert_eq!(choice.set("mitigations", Some("auto,nosmt")), Ok(true));
    for &(name, values) in FLAGS {
        if values.is_empty() {
            assert_eq!(choice.set(name, None), Ok(true), "{name}");
        }
        for &value in values {
            assert_eq!(choice.set(name, Some(value)), Ok(true), "{name}={value}");
        }
    }
}

#[test]
fn l1tf_guard_reaches_as_far_as_the_cache_bits() {
    // Skylake's cache holds 44 address bits, more than its 39 reach.
    let skylake = i5_6500();
    assert_eq!(skylake.cache_bits(), 44);
    let plan = default_plan(&skylake);
    assert_eq!(plan.swap_pages(), 1 << 31);
    assert_eq!(line(&plan, Flaw::L1tf), "Mitigation: PTE Inversion");
    // Skylake-SP's is as wide as its addresses.
    let server = Cpu { phys_bits: 46, ..gold_6152() };
    assert_eq!(server.cache_bits(), 46);
    assert_eq!(default_plan(&server).swap_pages(), 1 << 33);
    // Processors free of L1TF limit no swap area.
    assert_eq!(default_plan(&zen3()).swap_pages(), u64::MAX);
    // A processor that does not say gets Linux's 36 bits.
    assert_eq!(Cpu { phys_bits: 0, ..gold_6152() }.cache_bits(), 36);
}

#[test]
fn l1tf_is_open_with_memory_in_the_upper_half() {
    let cpu = Cpu { phys_bits: 36, memory_end: 32 << 30, ..gold_6152() };
    assert_eq!(line(&default_plan(&cpu), Flaw::L1tf), "Mitigation: PTE Inversion");
    let cpu = Cpu { memory_end: (32 << 30) + 4096, ..cpu };
    let plan = default_plan(&cpu);
    assert_eq!(line(&plan, Flaw::L1tf), "Vulnerable");
    assert!(plan.is_vulnerable(Flaw::L1tf));
}
