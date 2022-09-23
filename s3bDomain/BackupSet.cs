﻿using System;
using System.Collections.Generic;
using System.Text;
using System.Reflection;

namespace s3b
{
    public class BackupSet :  Model
    {
        public BackupSet()
        {
            tableName = "backup_set";
        }

        
        public List<LocalFolder> workFolders = new List<LocalFolder>();
        public Dictionary<long, LocalFolder> localFolders = new Dictionary<long, LocalFolder>();
        private Dictionary<string, LocalFolder> _uploadedFolders = new Dictionary<string, LocalFolder>();

        public Dictionary<string, LocalFolder> getUploadedFolders()
        {
            _uploadedFolders.Clear();

            foreach( LocalFolder fldr in localFolders.Values)
            {
                if (!_uploadedFolders.ContainsKey( fldr.encrypted_file_name ))
                {
                    _uploadedFolders.Add(fldr.encrypted_file_name, fldr);
                }
            }

            return _uploadedFolders;
        }

        public long id
        {
            get
            {
                return Convert.ToInt32(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }
        public string root_folder_path
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public string upload_target
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

    }
}
