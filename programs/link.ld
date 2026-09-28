/* Linker script dos programas de usuário do os-rust.
 *
 * A base 0x4000_0000 (1 GiB) fica no início da região do usuário
 * [0x4000_0000, 0x8000_0000) definida em SYSCALLS.md. Precisa ser
 * menor que 2 GiB porque o code model padrão do Rust gera relocações
 * R_X86_64_32S (endereços absolutos de 32 bits com sinal): uma base mais
 * alta faz o linker falhar com "relocation R_X86_64_32 out of range".
 *
 * Cada grupo de seções começa em uma página de 4 KiB própria, para que
 * nenhum PT_LOAD compartilhe página com outro: o carregador do kernel
 * mapeia página a página, com permissões por segmento (W^X), e recusa
 * segmentos que dividam uma página. */
ENTRY(_start)

SECTIONS {
    . = 0x40000000;

    .text : { *(.text .text.*) }

    . = ALIGN(4096);
    .rodata : { *(.rodata .rodata.*) *(.eh_frame_hdr) *(.eh_frame) }

    . = ALIGN(4096);
    .data : { *(.data .data.*) }

    .bss : { *(.bss .bss.*) *(COMMON) }
}
