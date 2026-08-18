// happy-path check on the rebooted machine: encode should return status 0
use baracuda_nvjpeg_sys as nvjpeg;
use std::ptr;

fn main() {
    let lib = nvjpeg::nvjpeg().unwrap();
    unsafe {
        let mut handle = ptr::null_mut();
        let mut state = ptr::null_mut();
        let mut est = ptr::null_mut();
        let mut params = ptr::null_mut();
        lib.nvjpeg_create_simple().unwrap()(&mut handle);
        lib.nvjpeg_jpeg_state_create().unwrap()(handle, &mut state);
        lib.nvjpeg_encoder_state_create().unwrap()(handle, &mut est, ptr::null_mut());
        lib.nvjpeg_encoder_params_create().unwrap()(handle, &mut params, ptr::null_mut());

        let src = std::fs::read("/tmp/big-photo.jpg").unwrap();
        let mut nc: i32 = 0;
        let mut ss: i32 = 0;
        let mut ws = [0i32; 3];
        let mut hs = [0i32; 3];
        let st = lib.nvjpeg_get_image_info().unwrap()(
            handle, src.as_ptr(), src.len(), &mut nc, &mut ss, ws.as_mut_ptr(), hs.as_mut_ptr(),
        );
        println!("get_image_info: {st:?} dims {}x{}", ws[0], hs[0]);
        let (w, h) = (ws[0] as usize, hs[0] as usize);
        let mut dec = vec![0u8; w * h * 3];
        let mut dimg = nvjpeg::nvjpegImage_t {
            channel: [dec.as_mut_ptr(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut()],
            pitch: [w * 3, 0, 0, 0],
        };
        let st = lib.nvjpeg_decode().unwrap()(
            handle, state, src.as_ptr(), src.len(), nvjpeg::nvjpegOutputFormat_t::Rgbi,
            &mut dimg, ptr::null_mut(),
        );
        println!("decode: {st:?}");

        lib.nvjpeg_encoder_params_set_quality().unwrap()(params, 50, ptr::null_mut());
        lib.nvjpeg_encoder_params_set_sampling_factors().unwrap()(
            params, nvjpeg::nvjpegChromaSubsampling_t::Css420, ptr::null_mut(),
        );
        lib.nvjpeg_encoder_params_set_optimized_huffman().unwrap()(params, 1, ptr::null_mut());
        let eimg = nvjpeg::nvjpegImage_t {
            channel: [dec.as_mut_ptr(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut()],
            pitch: [w * 3, 0, 0, 0],
        };
        let st = lib.nvjpeg_encode_image().unwrap()(
            handle, est, params, &eimg, 5, w as i32, h as i32, ptr::null_mut(),
        );
        println!("encode: {st:?} (expect 0)");
        if st.0 == 0 {
            let mut needed = 0usize;
            lib.nvjpeg_encode_retrieve_bitstream().unwrap()(
                handle, est, ptr::null_mut(), &mut needed, ptr::null_mut(),
            );
            let mut buf = vec![0u8; needed];
            let mut len = needed;
            lib.nvjpeg_encode_retrieve_bitstream().unwrap()(
                handle, est, buf.as_mut_ptr(), &mut len, ptr::null_mut(),
            );
            buf.truncate(len);
            std::fs::write("/tmp/gpu-encoded.jpg", &buf).unwrap();
            println!("wrote /tmp/gpu-encoded.jpg {len} bytes");
        }
    }
    println!("ALL DONE");
}
