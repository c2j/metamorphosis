select /*+ use_cplan*/ count(1)
   into total_num
   from (
SELECT t.client_acnt_id, t.sys_acnt_id, t.fund_code, t.accno, t.accname,
 t.accnamefund, t.belong_bank_code, t.coin_code, t.zone_code, t.brno,
 t.acnt_type, t.bank_name, t.bank_code, t.bank_cexc, t.bank_bic,
 t.sys_flag, t.cnt_flag, t.dept_code, t.dept_type,
 t.auth_area, t.asset_type, t.accname_eng, '8' AS sub_src_type,
 t.vald_flag, t.inure_begin_date, t.inure_end_date, t.parent_acnt_id, t.sysupdatetm,e.asset_acnt_id
FROM v_par_client_acnt_info_noflag t, v_acnt_check_base_rule e
WHERE e.client_acnt_id = t.client_acnt_id and t.if_inter_bank = '2'
) temp
   left join par_fund_info fi
    on temp.fund_code = fi.fund_code
   left join (select t.area_name,t.area_code
       from par_sys_area t
       where to_char(now(), 'yyyymmdd') between t.inure_begin_date and
            t.inure_end_date) sysarea
   on fi.area_code = sysarea.area_code
   WHERE temp.sub_src_type = '8'
    AND EXISTS (SELECT /*+ no_expand */ 1
                FROM MV_ACCOUNT_PRIV v
               WHERE v.account_code = temp.asset_acnt_id
                 AND v.user_id = p_i_user_id
                 AND v.role = p_i_role_id)
    and (p_i_qry_acnt is null or temp.accno = p_i_qry_acnt)
    and (p_i_qry_bank_pset is null or temp.accno = p_i_qry_bank_pset)
    and (p_i_qry_sys_flag is null or temp.sys_flag= p_i_qry_sys_flag)
    and (p_i_qry_vald_flag is null or temp.vald_flag = p_i_qry_vald_flag)
    and (p_i_qry_asset_type is null or temp.asset_type = p_i_qry_asset_type)
    and (p_i_qry_bank_name is null or temp.bank_name like '%' || p_i_qry_bank_name || '%')
    and (p_i_qry_area_code is null or sysarea.area_code = p_i_qry_area_code);
