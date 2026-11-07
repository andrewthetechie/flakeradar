use std::alloc::{GlobalAlloc,Layout,System};
use std::sync::atomic::{AtomicUsize,Ordering::Relaxed};
static LIVE:AtomicUsize=AtomicUsize::new(0);static PEAK:AtomicUsize=AtomicUsize::new(0);
const CAP:usize=3*1024*1024*1024;
struct A;
unsafe impl GlobalAlloc for A{
 unsafe fn alloc(&self,l:Layout)->*mut u8{let v=LIVE.fetch_add(l.size(),Relaxed)+l.size();PEAK.fetch_max(v,Relaxed);
  if v>CAP{ let m=b"EXCEEDED 3 GiB of live heap\n"; unsafe{libc_write(2,m.as_ptr(),m.len());libc_exit(42)} }
  unsafe{System.alloc(l)}}
 unsafe fn dealloc(&self,p:*mut u8,l:Layout){LIVE.fetch_sub(l.size(),Relaxed);unsafe{System.dealloc(p,l)}}
}
unsafe extern "C"{ #[link_name="write"] fn libc_write(fd:i32,p:*const u8,n:usize)->isize; #[link_name="_exit"] fn libc_exit(c:i32)->!; }
#[global_allocator] static G:A=A;
fn main(){
 let p=std::env::args().nth(1).unwrap();let b=std::fs::read(&p).unwrap();
 LIVE.store(0,Relaxed);PEAK.store(0,Relaxed);
 let t=std::time::Instant::now();
 let r=flakeradar_core::parsing::parse_junit_xml(&b,1);
 let peak=PEAK.load(Relaxed);
 let d=match &r{Ok(c)=>format!("Ok({} cases)",c.len()),Err(e)=>{let s=e.to_string();s.chars().take(200).collect()}};
 println!("{p}: body={} peak={} MiB budget=512 MiB elapsed={:?} result={d}",b.len(),peak/1024/1024,t.elapsed());
}
