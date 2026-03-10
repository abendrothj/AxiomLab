# **AxiomLab: Formally Verified Low-Level Runtimes for Autonomous Scientific Discovery**

## **Introduction to the Systems-Level Shift in Autonomous Science**

The landscape of computer science and artificial intelligence has reached a critical inflection point in 2026\. The discipline is witnessing a profound transition from artificial intelligence functioning as a passive generative tool to the deployment of autonomous, multi-agent systems operating directly in physical environments. While top-tier artificial intelligence labs have demonstrated success in using reasoning models to solve abstract mathematical problems, the deployment of these "agentic scientists" into physical, self-driving laboratories requires interacting with the messy reality of the hardware-software boundary.  
The future of autonomous scientific research relies heavily on systems programming. Artificial intelligence agents must control complex laboratory hardware—ranging from mass spectrometers and electron microscopes to high-throughput robotic liquid handlers—while simultaneously executing massive parallelized data analysis workflows. Historically, scientific computing has relied on C, C++, and Fortran, which prioritize raw execution speed but offer zero memory safety guarantees and are notoriously difficult to formally verify. As we hand over the autonomous control of chemical synthesis and physical experimentation to artificial intelligence, the reliance on memory-unsafe and unverified low-level systems poses an unacceptable physical and computational risk.  
This report delineates a cutting-edge, PhD-level research domain that sits at the nexus of artificial intelligence, low-level systems programming, and formal verification. The proposed dissertation topic—**AxiomLab: Formally Verified Low-Level Runtimes for Autonomous Scientific Discovery**—addresses the critical limitations of current autonomous lab frameworks by embedding AI-driven control logic within the Rust programming language, enhanced by state-of-the-art formal verification tools like Verus, Aeneas, and Flux. By synthesizing agentic workflows with memory-safe, formally verified hardware and compute interactions, this research paradigm offers a robust solution to the pervasive issues of memory corruption, concurrency faults, and floating-point instability. It provides a definitive, bare-metal architecture for the next generation of self-driving laboratories.

## **The Research Gap: Why This is Untouched Territory**

To ensure this dissertation represents truly novel, PhD-level work, an analysis of the current state-of-the-art literature across formal verification and agentic science was conducted. This analysis confirms that the proposed intersection is currently an untouched frontier. No other research group has successfully deployed formally verified Rust systems to govern the physical actions of AI agents in autonomous laboratories.  
The novelty of this dissertation is defined by bridging three distinct, currently isolated research silos:

1. **Verification is Currently Confined to Digital Systems:** The state-of-the-art in Rust formal verification is heavily focused on pure software infrastructure. For instance, the Verus compiler is actively being used to verify distributed systems, concurrent memory allocators, and operating system page tables. Similarly, the Aeneas framework is currently being utilized by major tech companies to port the SymCrypt cryptographic library into verified Rust. However, these tools have never been adapted to verify *physical laboratory hardware constraints* or continuous scientific workflows.  
2. **Autonomous Labs are Stuck in Unverified Runtimes:** Current self-driving laboratories rely almost entirely on Python-based orchestration frameworks (such as Node-RED or the Robot Operating System), which offer no formal safety guarantees and are highly vulnerable to agent hallucinations that cause memory faults.  
3. **AI Proof Synthesis Lacks Physical Grounding:** While new LLM-based verification agents like VeruSAGE are pushing the boundaries of automated proof synthesis in Rust, their benchmarks are strictly limited to classic computer science algorithms and standard repository-level software.

By creating a framework where Verus and Aeneas are used to mathematically prove that an AI agent's generated code will not violate the physical boundaries of a laboratory robot or the thermodynamic invariants of a chemical synthesis process, this dissertation will be the absolute first of its kind.

## **The Crisis of Safety: Memory Corruption, Concurrency, and Numeric Drift**

The deployment of large language models across complex, high-stakes physical domains has exposed severe vulnerabilities in how these systems interface with low-level execution environments. Current artificial intelligence agents acting in physical spaces frequently fall victim to the "reasoning-action dilemma." Despite possessing advanced abstract reasoning capabilities, these agents often fail dramatically when their generated code interacts with underlying system hardware.  
When translated to the domain of autonomous scientific discovery, these failures are catastrophic. The promise of self-driving laboratories relies entirely on the programmatic consistency of the underlying computational agents. If an AI agent directing a robotic chemistry lab or a materials acceleration platform generates an off-by-one error in a C-based memory buffer, or triggers a data race in a highly concurrent data analysis pipeline, the resulting experiments are not only invalid but potentially physically hazardous to the laboratory environment.  
Three primary low-level vulnerabilities plague the current integration of AI and scientific computing:

1. **Memory Un-safety:** The vast majority of legacy scientific computing libraries (e.g., LAPACK, OpenBLAS) are written in C or Fortran. When AI agents write integration scripts or execute system calls against these libraries, they are highly prone to triggering buffer overflows, use-after-free errors, and segmentation faults, causing the entire autonomous loop to crash.  
2. **Concurrency Faults:** High-performance scientific discovery requires massively parallel execution. AI agents struggle to write provably safe concurrent code, often introducing data races and deadlocks when managing threads for complex simulations or hardware polling.  
3. **Floating-Point Instability:** Formal verification of floating-point arithmetic remains one of the most notoriously difficult challenges in computer science due to non-linear arithmetic behavior and the tight coupling between control and datapath logic. When AI agents optimize mathematical equations, subtle semantic discrepancies in floating-point round-offs can compound over long-running simulations, invalidating the scientific results.

There is an urgent need for an architectural paradigm shift that replaces unverified, memory-unsafe execution with deterministic, formally verified low-level runtimes.

## **Rust and Liquid Types: The Foundation of Safe Scientific Compute**

To circumvent the limitations of C and Fortran, the scientific computing community is actively migrating toward Rust. Rust provides low-level control over hardware—essential for operating laboratory robotics—while enforcing strict memory safety and thread safety at compile-time through its unique ownership and borrowing model.  
The emergence of comprehensive, pure-Rust scientific ecosystems proves that high-performance computing no longer requires unsafe legacy dependencies. For instance, the SciRS2 ecosystem provides a completely self-contained, 100% pure Rust implementation for scientific computing, including linear algebra (OxiBLAS) and fast Fourier transforms (OxiFFT), achieving performance that rivals or exceeds traditional C/Fortran libraries without relying on external system dependencies. When an AI agent executes data analysis or hypothesis generation using these pure-Rust crates, entire classes of memory corruption bugs are fundamentally mathematically eliminated.  
However, Rust's standard type system cannot eliminate logic errors, such as array bounds violations, integer overflows, or physical invariant violations in scientific models. To achieve absolute certainty in autonomous environments, the dissertation must leverage advanced type theory, specifically **Liquid Types**.  
The Flux verifier enhances Rust's type system with logical refinements, enabling the specification and automated verification of rich program invariants. By leveraging syntax-directed typing rules and SMT-based reasoning, Flux can statically prove properties that lie beyond the guarantees of Rust's native compiler. For an autonomous laboratory agent, this means that before a generated Rust kernel is compiled and sent to a robotic arm, Flux can mathematically guarantee that the system will never violate predefined spatial boundaries or resource constraints, ensuring physical safety at the type level.

## **Systems-Level Formal Verification: Verus and Aeneas**

While Liquid Types provide inline safety guarantees, fully verifying the functional correctness of complex, highly concurrent low-level systems requires dedicated formal verification frameworks. The definitive tools for this endeavor in the Rust ecosystem are Verus and Aeneas.  
**Verus** is a state-of-the-art tool designed specifically for verifying the correctness of low-level systems code written in Rust. Verification is entirely static; Verus adds no run-time overhead, instead using the Z3 SMT solver to statically verify that the executable Rust code will always satisfy user-provided specifications for all possible executions. Verus utilizes "linear ghost variables"—proof-time variables that are not compiled but retain the unique-ownership and safe reference rules of the borrow checker. This allows developers (and AI agents) to efficiently reason about interior mutability and shared-memory concurrency, which are vital for multi-threaded scientific simulations and hardware control. Recent advancements have demonstrated Verus's capability to verify incredibly complex systems, including concurrent memory allocators and distributed systems, executing 3 to 61 times faster than prior state-of-the-art verification tools.  
**Aeneas** provides an alternative, equally powerful verification path. Aeneas is a verification toolchain that translates Rust's MIR (Mid-level Intermediate Representation) into a pure lambda calculus, enabling verification of functional correctness using interactive theorem provers like Lean 4 or Coq. The genius of Aeneas is its ability to leverage Rust's strict aliasing rules to translate complex, stateful memory operations into purely functional representations. This entirely eliminates the burden of complex memory reasoning (such as separation logic) within the theorem prover. Aeneas is actively being used in industry to port highly critical, heavily optimized C code (such as cryptographic libraries) into verified Rust. In an autonomous lab, Aeneas allows researchers to mathematically prove that the high-performance data processing pipelines generated by an AI agent functionally compute the exact equations they claim to.

| Framework | Core Mechanism | Scientific Lab Application |
| :---- | :---- | :---- |
| **Flux** | Liquid Types / Refinement Types | Enforcing physical array bounds and numeric constraints at compile time. |
| **Verus** | SMT-based Functional Verification | Proving the safety of concurrent laboratory hardware control and resource allocators. |
| **Aeneas** | Rust MIR to Lean 4 Translation | Translating highly optimized data pipelines into pure logic to prove absolute mathematical correctness. |

## **LLM-Assisted Proof Synthesis for Systems Code**

The manual formalization of low-level systems code is an extraordinarily labor-intensive process, traditionally requiring specialized expertise. However, the emergence of AI-driven proof synthesis is automating this bottleneck.  
Recent research, such as the **VeruSAGE** framework, demonstrates that Large Language Models (LLMs) can be integrated into sophisticated agentic systems to automatically develop correctness proofs for system software written in Rust. By structuring an observation-reasoning-action loop, the VeruSAGE agent mimics human experts by executing preliminary proof generation, applying generic refinement tips, and debugging code guided strictly by Verus compiler errors. Empirical results show that optimal LLM-agent combinations can autonomously complete over 80% of complex system-verification tasks. Furthermore, automated synthesis pipelines like VeruSyn have successfully fine-tuned models specifically for generating long-chain-of-thought proofs that satisfy Verus.  
This creates a powerful "ouroboros" of automated science: An AI scientific agent writes a highly optimized, low-level Rust script to execute a novel experiment. Simultaneously, an AI verification agent (like VeruSAGE) writes the formal mathematical proofs required by Verus or Aeneas to guarantee the script's safety and functional correctness. If the proof fails, the system refuses to compile the code, entirely preventing the "hallucination" of dangerous physical actions.

## **Detailed GitHub Implementation Plan: Building the Verified Runtime**

To realize this architecture, the dissertation will produce a modular, open-source GitHub repository named **AxiomLab**.  
**Repository Description:** *A bare-metal, memory-safe, and formally verified Rust runtime for autonomous AI scientists and self-driving laboratories.*  
The implementation strategy involves building an end-to-end framework where AI-generated scientific operations are sandboxed, structurally validated, and mathematically verified before execution.  
The repository will be structured into four core phases, effectively serving as the compiler pipeline for autonomous scientific code.

### **Phase 1: Pure Rust Scientific Primitives and Dimensional Analysis**

The first step is establishing a computation layer free of unsafe memory abstractions. Traditional AI architectures rely on Python wrapped around C libraries, making formal verification nearly impossible.

* **Action:** Implement the base data-processing modules using the SciRS2 ecosystem. Because SciRS2 is 100% pure Rust and requires zero C/C++/Fortran dependencies for linear algebra and fast Fourier transforms, the baseline code generated by the agent avoids entire classes of memory corruption from the start.  
* **Action:** Integrate compile-time dimensional analysis using the uom (units of measurement) or dimensioned crates. If an AI agent hallucinates and attempts to add a mass value to a velocity value—an action that is physically meaningless—the uom crate will trigger a strict compile-time type error, halting the execution before it reaches the verification stage.

### **Phase 2: Agent Orchestration and Sandboxing**

The LLM must be embedded within a highly restricted control plane to manage its interactions with the laboratory hardware.

* **Action:** Build the agent orchestrator using a minimal systems-level framework like ZeroClaw. ZeroClaw provides a fast, single-binary Rust runtime with strict workspace sandboxing, explicit allowlists, and a low memory footprint. This ensures the agent cannot execute rogue system commands outside of its designated hardware-control workspace.

### **Phase 3: Concurrent Safety and Hardware Verification (Verus)**

When the AI agent generates concurrent code to control physical hardware (e.g., polling sensors while driving a robotic arm), the system must prove that no data races, deadlocks, or out-of-bounds array accesses can occur.

* **Action:** Integrate the Verus compiler into the toolchain. Use Verus's SMT-based reasoning and linear ghost variables to define the limits of the physical hardware (e.g., maximum robotic arm extension) as mathematical specifications.  
* **Action:** Implement a proof-synthesis agent based on the VeruSAGE methodology. When the primary AI scientist proposes a Rust script, the VeruSAGE-inspired agent will enter an observation-reasoning-action loop, automatically generating the necessary Verus proof blocks. If Verus rejects the proof, the agent intercepts the compiler error and self-corrects the hardware script.

### **Phase 4: Algorithmic Verification (Aeneas and Lean 4\)**

For the most critical scientific optimizations (e.g., the underlying chemical equations governing a synthesis), the system must guarantee absolute mathematical correctness beyond memory safety.

* **Action:** Utilize the Aeneas toolchain to translate the validated Rust MIR (Mid-level Intermediate Representation) into a pure lambda calculus.  
* **Action:** Export this functional model directly into Lean 4\. This allows the AI agent to utilize theorem-proving frameworks (such as LeanDojo) to strictly verify that the high-performance Rust execution pipeline accurately computes the intended continuous physics models without floating-point errors or logical drifts.

### **Repository Directory Structure**

├── /agent\_runtime/ \# Powered by ZeroClaw for sandboxed, low-memory execution  
├── /scientific\_compute/ \# Pure Rust physics and math using SciRS2  
├── /physical\_types/ \# Compile-time dimensional analysis via the 'uom' crate  
├── /verus\_proofs/ \# Concurrency and hardware invariant SMT proofs  
├── /proof\_synthesizer/ \# VeruSAGE-inspired agent for automated proof generation  
└── /aeneas\_lean\_semantics/ \# Rust MIR translations to Lean 4 for algorithmic verification  
By adhering to this implementation plan, the resulting open-source repository will bridge the gap between AI autonomy and absolute physical safety, delivering a blistering fast, production-ready operating system for the next generation of self-driving laboratories.