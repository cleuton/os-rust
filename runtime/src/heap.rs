//! O alocador global dos programas: o que faz `Box`, `Vec` e `String`
//! funcionarem num programa de usuário.
//!
//! É o mesmo `linked_list_allocator` que o kernel usa (`src/allocator.rs`);
//! só muda de onde vem a memória. O kernel tem um heap fixo, pré-mapeado no
//! boot; aqui o heap **começa vazio** e cresce sob demanda com a syscall
//! `SYS_ALLOC`. Cada pedido de crescimento vem contíguo ao anterior (o
//! contrato garante, `SYSCALLS.md` seção 5), então um único bloco de memória
//! basta: o alocador só o estende.

use core::alloc::{GlobalAlloc, Layout};

use linked_list_allocator::LockedHeap;

use crate::sys;

/// Menor crescimento pedido ao kernel de uma vez (16 KiB), para não fazer uma
/// syscall a cada `Box` pequeno.
const MIN_GROWTH: usize = 16 * 1024;

/// Tamanho de página do kernel: o `SYS_ALLOC` arredonda para múltiplos disso.
const PAGE_SIZE: usize = 4096;

/// O heap do programa, com o crescimento por `SYS_ALLOC` por cima.
struct Heap(LockedHeap);

// SAFETY: `alloc` só devolve ponteiros que `LockedHeap` entregou (memória
// mapeada e exclusiva do programa, obtida de `SYS_ALLOC`) e nunca entrega o
// mesmo bloco duas vezes; `dealloc` só repassa o bloco ao mesmo `LockedHeap`
// que o entregou, com o mesmo `layout`, como o contrato de `GlobalAlloc` pede.
unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` vem do chamador de `GlobalAlloc::alloc`, que já
        // garante que ele é válido (tamanho não nulo, alinhamento potência de 2).
        let ptr = unsafe { self.0.alloc(layout) };
        if !ptr.is_null() {
            return ptr;
        }

        // Sem espaço: pede mais memória ao kernel. A folga de `align` cobre o
        // alinhamento pedido; o mínimo evita syscalls demais.
        let Some(need) = layout.size().checked_add(layout.align()) else {
            return core::ptr::null_mut();
        };
        let Some(rounded) = need.checked_add(PAGE_SIZE - 1) else {
            return core::ptr::null_mut();
        };
        let growth = (rounded & !(PAGE_SIZE - 1)).max(MIN_GROWTH);

        let start = sys::alloc(growth);
        if start < 0 {
            // `ERR_NOMEM` (ou outro erro): sem memória. Devolver nulo faz o
            // `alloc` do Rust chamar o tratador de falha de alocação, que vira
            // um `panic` (ver `panic` em `lib.rs`).
            return core::ptr::null_mut();
        }

        {
            let mut heap = self.0.lock();
            if heap.size() == 0 {
                // SAFETY: `start..start + growth` é memória que o kernel acabou
                // de dar ao programa (mapeada, zerada, gravável), e nenhum
                // outro código a usa; é a primeira e única inicialização.
                unsafe { heap.init(start as *mut u8, growth) };
            } else {
                // SAFETY: a área nova começa exatamente onde o heap termina
                // (contiguidade garantida pelo contrato de `SYS_ALLOC`) e é
                // memória exclusiva do programa.
                unsafe { heap.extend(growth) };
            }
        }
        // SAFETY: mesma garantia do primeiro `alloc` acima.
        unsafe { self.0.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` e `layout` são os de um `alloc` anterior deste mesmo
        // alocador, como `GlobalAlloc::dealloc` exige do chamador.
        unsafe { self.0.dealloc(ptr, layout) }
    }
}

/// O alocador global de todo programa que usa a biblioteca. Se o programa
/// nunca aloca, o linker o descarta junto com o resto do que não é usado.
#[global_allocator]
static HEAP: Heap = Heap(LockedHeap::empty());
